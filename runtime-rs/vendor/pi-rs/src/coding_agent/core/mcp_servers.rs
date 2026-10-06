//! Port of upstream `coding-agent/src/core/mcp-servers.ts`: MCP server
//! configuration (the `mcpServers` shape) and the registry behind
//! `pi.registerMcpServer()`. The core only validates and stores
//! registrations; the MCP extension (built in, or another extension handling
//! `mcp_servers_change`) connects them next to the servers from `mcp.json`.
//!
//! Configs keep their original key order (upstream spreads the raw object,
//! and `toolExposure` pattern precedence is "first match in the object"), so
//! the validated config carries an insertion-ordered map.

use serde_json::Value;

use crate::ai::types::ordered_map::OrderedMap;

/// Upstream `McpExposure`.
///
/// - `codemode`: tools are callable from codemode scripts but neither
///   declared to the model nor listed in the codemode description, which
///   lists only the server's namespace. Scripts find them with
///   `searchTools()`.
/// - `deferred`: not declared to the model until the `tool_search` tool loads
///   them; the model then calls them directly. Does not need codemode.
/// - `direct`: tools are declared to the model like any other tool (and
///   callable from codemode).
/// - `hidden`: tools are registered but unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpExposure {
    Codemode,
    Deferred,
    Direct,
    Hidden,
}

impl McpExposure {
    fn as_str(&self) -> &'static str {
        match self {
            McpExposure::Codemode => "codemode",
            McpExposure::Deferred => "deferred",
            McpExposure::Direct => "direct",
            McpExposure::Hidden => "hidden",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "codemode" | "codemode-deferred" => Some(McpExposure::Codemode),
            "deferred" => Some(McpExposure::Deferred),
            "direct" => Some(McpExposure::Direct),
            "hidden" => Some(McpExposure::Hidden),
            _ => None,
        }
    }
}

const MCP_EXPOSURES: &[&str] = &["codemode", "deferred", "direct", "hidden"];

const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "[::1]"];

/// Whether a redirect URI can be served by pi's loopback callback server.
pub fn is_loopback_redirect_uri(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http"
        && LOOPBACK_HOSTS.contains(&url.host_str().unwrap_or_default())
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Upstream `McpOAuthConfig`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpOAuthConfig {
    /// Pre-registered client id. Without it, pi registers a client with the
    /// authorization server.
    pub client_id: Option<String>,
    /// May reference environment variables (`${NAME}`) or commands (`!cmd`).
    pub client_secret: Option<String>,
    /// Port of the loopback callback server, for clients registered with a
    /// fixed redirect URI.
    pub callback_port: Option<u16>,
    /// Redirect URI registered for `clientId`, for example
    /// `http://localhost:8080/oauth/callback`. Must be an `http` URI on
    /// `localhost`, `127.0.0.1`, or `[::1]`. Without a port, the callback
    /// server listens on `callbackPort` or a free port, which is added to the
    /// URI (RFC 8252).
    pub callback_url: Option<String>,
    /// Scopes to request, separated by spaces. Default: the scopes the server
    /// advertises.
    pub scope: Option<String>,
    /// `client_name` sent with dynamic client registration. Default: `pi`.
    pub client_name: Option<String>,
    /// How pi identifies itself without `clientId` (v1.0.0). `dcr` (default):
    /// dynamic client registration. `cimd`: pi's Client ID Metadata Document on
    /// pi.dev.
    pub client_registration: Option<McpClientRegistration>,
    /// Authorization server metadata document (RFC 8414 or OpenID Connect
    /// discovery) to use instead of discovery through the server (v1.0.0).
    /// Must use https, except on loopback hosts.
    pub auth_server_metadata_url: Option<String>,
}

/// Upstream `McpOAuthConfig.clientRegistration` (v1.0.0): how pi identifies
/// itself without `clientId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientRegistration {
    /// `"dcr"` (default): dynamic client registration.
    Dcr,
    /// `"cimd"`: pi's Client ID Metadata Document on pi.dev, for authorization
    /// servers that allow pi by that URL. The server must support it for public
    /// clients, and the callback must use the default path `/callback`.
    Cimd,
}

impl McpClientRegistration {
    pub fn as_str(self) -> &'static str {
        match self {
            McpClientRegistration::Dcr => "dcr",
            McpClientRegistration::Cimd => "cimd",
        }
    }
}

/// One validated server entry. The transport lives in the raw map (upstream
/// returns the raw object cast to the interface — no type normalization, so
/// `type: "streamable-http"` survives verbatim).
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    /// The validated entry with exposure aliases resolved, in original key
    /// order.
    raw: OrderedMap<Value>,
    exposure: Option<McpExposure>,
    tool_exposure: OrderedMap<McpExposure>,
}

impl McpServerConfig {
    /// The server `exposure` (validated already).
    pub fn exposure(&self) -> McpExposure {
        self.exposure.unwrap_or(McpExposure::Codemode)
    }

    /// The `toolExposure` overrides, in config order.
    pub fn tool_exposure(&self) -> &OrderedMap<McpExposure> {
        &self.tool_exposure
    }

    /// The validated entry, in original key order — the `{ ...config }` spread
    /// base for project overrides and re-save merges (upstream spreads the
    /// validated config object itself).
    pub fn raw(&self) -> &OrderedMap<Value> {
        &self.raw
    }

    fn field(&self, key: &str) -> Option<&Value> {
        self.raw.get(key).filter(|value| !value.is_null())
    }

    /// `command` (stdio transport).
    pub fn command(&self) -> Option<&str> {
        self.field("command").and_then(Value::as_str)
    }

    /// `url` (streamable HTTP transport).
    pub fn url(&self) -> Option<&str> {
        self.field("url").and_then(Value::as_str)
    }

    /// The transport `type` as written (no normalization, upstream keeps
    /// `streamable-http`).
    pub fn server_type(&self) -> Option<&str> {
        self.field("type").and_then(Value::as_str)
    }

    /// `args` (stdio transport).
    pub fn args(&self) -> Option<Vec<&str>> {
        self.field("args")
            .and_then(Value::as_array)
            .map(|args| args.iter().filter_map(Value::as_str).collect())
    }

    /// `env` / `headers`: name → value records (`${NAME}` / `!cmd` refs).
    pub fn string_record(&self, key: &str) -> Option<Vec<(String, String)>> {
        self.field(key).and_then(Value::as_object).map(|record| {
            record
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
    }

    /// `cwd` (stdio transport).
    pub fn cwd(&self) -> Option<&str> {
        self.field("cwd").and_then(Value::as_str)
    }

    /// `oauth` (HTTP transport).
    pub fn oauth(&self) -> Option<McpOAuthConfig> {
        let oauth = self.field("oauth")?;
        let object = oauth.as_object()?;
        let string_field = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
        Some(McpOAuthConfig {
            client_id: string_field("clientId"),
            client_secret: string_field("clientSecret"),
            callback_port: object
                .get("callbackPort")
                .and_then(Value::as_u64)
                .map(|port| port as u16),
            callback_url: string_field("callbackUrl"),
            scope: string_field("scope"),
            client_name: string_field("clientName"),
            client_registration: object
                .get("clientRegistration")
                .and_then(Value::as_str)
                .and_then(|value| match value {
                    "dcr" => Some(McpClientRegistration::Dcr),
                    "cimd" => Some(McpClientRegistration::Cimd),
                    _ => None,
                }),
            auth_server_metadata_url: string_field("authServerMetadataUrl"),
        })
    }

    /// `auth.provider` (v1.0.0): the pi provider whose token replaces MCP
    /// OAuth for this server.
    pub fn auth_provider(&self) -> Option<&str> {
        self.field("auth")
            .and_then(Value::as_object)
            .and_then(|auth| auth.get("provider"))
            .and_then(Value::as_str)
    }

    /// `enabled`: set to false to keep the entry without connecting.
    pub fn enabled(&self) -> bool {
        self.field("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// Per-request timeout in seconds.
    pub fn timeout(&self) -> Option<f64> {
        self.field("timeout").and_then(Value::as_f64)
    }

    /// What the server offers, in a sentence.
    pub fn description(&self) -> Option<&str> {
        self.field("description").and_then(Value::as_str)
    }
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

fn is_string_record(value: &Value) -> bool {
    value
        .as_object()
        .map(|record| record.values().all(Value::is_string))
        .unwrap_or(false)
}

const SERVER_NAME_OK: fn(&str) -> bool = |name| {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
};

/// Namespace of a server's tools: `mcp__<server>` with `-` replaced by `_`,
/// like the tool names (upstream `mcpNamespace`, v1.0.0).
pub fn mcp_namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

fn validate_oauth(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    if !is_record(value) {
        return Some("oauth must be an object".to_string());
    }
    let object = value.as_object().expect("record");
    let port = object.get("callbackPort");
    if let Some(port) = port {
        let valid = port
            .as_f64()
            .map(|port| port.fract() == 0.0 && (1.0..=65535.0).contains(&port))
            .unwrap_or(false);
        if !valid {
            return Some("oauth.callbackPort must be a port number".to_string());
        }
    }
    if let Some(callback_url) = object.get("callbackUrl") {
        if !callback_url.is_string() || !is_loopback_redirect_uri(callback_url.as_str().unwrap()) {
            return Some(
                "oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] \
                 without query or fragment"
                    .to_string(),
            );
        }
        let url = url::Url::parse(callback_url.as_str().unwrap()).expect("validated URL");
        let url_port = url.port();
        if let (Some(url_port), Some(port)) = (url_port, port.and_then(Value::as_f64)) {
            if f64::from(url_port) != port {
                return Some(
                    "oauth.callbackUrl and oauth.callbackPort name different ports".to_string(),
                );
            }
        }
    }
    if let Some(client_name) = object.get("clientName") {
        let empty = client_name
            .as_str()
            .map(|name| name.trim().is_empty())
            .unwrap_or(true);
        if !client_name.is_string() || empty {
            return Some("oauth.clientName must be a non-empty string".to_string());
        }
    }
    // v1.0.0: clientRegistration. `dcr` is the default and always accepted.
    if let Some(client_registration) = object.get("clientRegistration") {
        let registration = client_registration.as_str();
        if registration != Some("dcr") {
            if registration != Some("cimd") {
                return Some("oauth.clientRegistration must be \"dcr\" or \"cimd\"".to_string());
            }
            if object.get("clientId").is_some() || object.get("clientName").is_some() {
                return Some(
                    "oauth.clientRegistration \"cimd\" cannot be combined with oauth.clientId or oauth.clientName"
                        .to_string(),
                );
            }
            let callback = object
                .get("callbackUrl")
                .and_then(Value::as_str)
                .and_then(|value| url::Url::parse(value).ok());
            if let Some(callback) = callback {
                if callback.host_str() == Some("[::1]") || callback.path() != "/callback" {
                    return Some(
                        "oauth.clientRegistration \"cimd\" requires oauth.callbackUrl on localhost or 127.0.0.1 with path /callback"
                            .to_string(),
                    );
                }
            }
        }
    }
    // v1.0.0: authServerMetadataUrl — https, or http on a loopback host.
    if let Some(metadata_url) = object.get("authServerMetadataUrl") {
        let url = metadata_url
            .as_str()
            .and_then(|value| url::Url::parse(value).ok());
        let valid = url
            .map(|url| {
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && LOOPBACK_HOSTS.contains(&url.host_str().unwrap_or_default()))
            })
            .unwrap_or(false);
        if !valid {
            return Some(
                "oauth.authServerMetadataUrl must be an https URL, or http on localhost, 127.0.0.1, or [::1]"
                    .to_string(),
            );
        }
    }
    None
}

/// `URL.canParse` + protocol check for the transports that carry a URL.
fn parse_http_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    if matches!(url.scheme(), "http" | "https") {
        Some(url)
    } else {
        None
    }
}

/// A copy of the server entry with exposure aliases replaced by their current
/// names, preserving key order.
fn resolve_exposure_aliases(value: &OrderedMap<Value>) -> OrderedMap<Value> {
    let mut resolved = value.clone();
    if let Some(exposure) = value.get("exposure") {
        if let Some(text) = exposure.as_str() {
            if let Some(resolved_exposure) = McpExposure::parse(text) {
                resolved.insert(
                    "exposure",
                    Value::String(resolved_exposure.as_str().to_string()),
                );
            }
        }
    }
    if let Some(tool_exposure) = value.get("toolExposure").and_then(Value::as_object) {
        // serde_json's `preserve_order` keeps the entry order of the object.
        let mut resolved_tools = serde_json::Map::new();
        for (tool, entry) in tool_exposure {
            let entry = entry
                .as_str()
                .and_then(McpExposure::parse)
                .map(|exposure| Value::String(exposure.as_str().to_string()))
                .unwrap_or_else(|| entry.clone());
            resolved_tools.insert(tool.clone(), entry);
        }
        resolved.insert("toolExposure", Value::Object(resolved_tools));
    }
    resolved
}

fn tool_pattern_matches(pattern: &str, tool_name: &str) -> bool {
    let source = pattern
        .split('*')
        .map(|part| {
            let mut escaped = String::with_capacity(part.len());
            for character in part.chars() {
                if matches!(
                    character,
                    '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
                ) {
                    escaped.push('\\');
                }
                escaped.push(character);
            }
            escaped
        })
        .collect::<Vec<_>>()
        .join(".*");
    regex::Regex::new(&format!("^{source}$"))
        .map(|regex| regex.is_match(tool_name))
        .unwrap_or(false)
}

/// Exposure of one tool of a server: its `toolExposure` entry, else the
/// server's `exposure`.
pub fn get_mcp_tool_exposure(config: &McpServerConfig, tool_name: &str) -> McpExposure {
    let overrides = &config.tool_exposure;
    if let Some(exact) = overrides.get(tool_name) {
        return *exact;
    }
    for (pattern, exposure) in overrides.iter() {
        if pattern.contains('*') && tool_pattern_matches(pattern, tool_name) {
            return *exposure;
        }
    }
    config.exposure()
}

/// Validate one server entry of the `mcpServers` shape. Returns the config
/// with exposure aliases resolved, or the upstream error message.
pub fn validate_mcp_server_config(
    name: &str,
    raw: &OrderedMap<Value>,
) -> Result<McpServerConfig, String> {
    if !SERVER_NAME_OK(name) {
        return Err(format!(
            "invalid server name \"{name}\" (use letters, digits, \"_\" and \"-\")"
        ));
    }
    let value = resolve_exposure_aliases(raw);
    let exposure_value = value.get("exposure").cloned();
    let tool_exposure_value = value.get("toolExposure").cloned();
    let enabled = value.get("enabled").cloned();
    let timeout = value.get("timeout").cloned();
    let description = value.get("description").cloned();
    let exposures = MCP_EXPOSURES
        .iter()
        .map(|exposure| format!("\"{exposure}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let exposure = match &exposure_value {
        None => None,
        Some(text) if text.as_str().and_then(McpExposure::parse).is_some() => {
            McpExposure::parse(text.as_str().expect("string"))
        }
        Some(_) => {
            return Err(format!(
                "server \"{name}\": exposure must be one of {exposures}"
            ));
        }
    };
    let tool_exposure = match &tool_exposure_value {
        None => OrderedMap::new(),
        Some(entry) if is_record(entry) => {
            let mut resolved = OrderedMap::new();
            for (tool, value) in entry.as_object().expect("record") {
                match value.as_str().and_then(McpExposure::parse) {
                    Some(exposure) => {
                        resolved.insert(tool.clone(), exposure);
                    }
                    None => {
                        return Err(format!(
                            "server \"{name}\": toolExposure \"{tool}\" must be one of {exposures}"
                        ));
                    }
                }
            }
            resolved
        }
        Some(_) => {
            return Err(format!(
                "server \"{name}\": toolExposure must map tool names to exposures"
            ));
        }
    };
    if let Some(enabled) = &enabled {
        if !enabled.is_boolean() {
            return Err(format!("server \"{name}\": enabled must be a boolean"));
        }
    }
    if let Some(description) = &description {
        if !description.is_string() {
            return Err(format!("server \"{name}\": description must be a string"));
        }
    }
    if let Some(timeout) = &timeout {
        let valid = timeout
            .as_f64()
            .map(|timeout| timeout > 0.0)
            .unwrap_or(false);
        if !valid {
            return Err(format!(
                "server \"{name}\": timeout must be a positive number of seconds"
            ));
        }
    }
    if value.get("type").and_then(Value::as_str) == Some("sse") {
        return Err(format!(
            "server \"{name}\": legacy SSE transport is not supported; use the streamable HTTP URL"
        ));
    }

    let server_type = value.get("type").and_then(Value::as_str);
    if let Some(url) = value.get("url").and_then(Value::as_str) {
        if server_type.is_none() || matches!(server_type, Some("http") | Some("streamable-http")) {
            if parse_http_url(url).is_none() {
                return Err(format!(
                    "server \"{name}\": url must be an http or https URL"
                ));
            }
            if let Some(headers) = value.get("headers") {
                if !is_string_record(headers) {
                    return format_err(name, "headers must map names to strings");
                }
            }
            if let Some(oauth_error) = validate_oauth(value.get("oauth").unwrap_or(&Value::Null)) {
                return Err(format!("server \"{name}\": {oauth_error}"));
            }
            // v1.0.0: `auth` sends a pi provider's token instead of MCP OAuth.
            if let Some(auth) = value.get("auth") {
                let provider = auth
                    .as_object()
                    .and_then(|auth| auth.get("provider"))
                    .and_then(Value::as_str)
                    .filter(|provider| !provider.is_empty());
                if provider.is_none() {
                    return Err(format!(
                        "server \"{name}\": auth.provider must be a provider name"
                    ));
                }
                let url = url::Url::parse(url).expect("validated URL");
                let https = url.scheme() == "https";
                if !https && !LOOPBACK_HOSTS.contains(&url.host_str().unwrap_or_default()) {
                    return Err(format!(
                        "server \"{name}\": auth requires an https URL, or http on localhost, 127.0.0.1, or [::1]"
                    ));
                }
            }
            return Ok(McpServerConfig {
                raw: value,
                exposure,
                tool_exposure,
            });
        }
    }
    if let Some(command) = value.get("command").and_then(Value::as_str) {
        let _ = command;
        if server_type.is_none() || server_type == Some("stdio") {
            if let Some(args) = value.get("args") {
                let valid = args
                    .as_array()
                    .map(|args| args.iter().all(Value::is_string))
                    .unwrap_or(false);
                if !valid {
                    return format_err(name, "args must be an array of strings");
                }
            }
            if let Some(env) = value.get("env") {
                if !is_string_record(env) {
                    return format_err(name, "env must map names to strings");
                }
            }
            if let Some(cwd) = value.get("cwd") {
                if !cwd.is_string() {
                    return format_err(name, "cwd must be a string");
                }
            }
            return Ok(McpServerConfig {
                raw: value,
                exposure,
                tool_exposure,
            });
        }
    }
    Err(format!(
        "server \"{name}\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"
    ))
}

fn format_err(name: &str, message: &str) -> Result<McpServerConfig, String> {
    Err(format!("server \"{name}\": {message}"))
}

/// A server an extension registered with `pi.registerMcpServer()`.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredMcpServer {
    pub name: String,
    pub config: McpServerConfig,
    /// Path of the extension that registered the server.
    pub extension_path: String,
}

/// Servers registered by the extensions of one runtime.
#[derive(Default)]
pub struct McpServerRegistry {
    /// Registration order (upstream `Map` insertion order).
    servers: Vec<RegisteredMcpServer>,
    change_listener: Option<Box<dyn Fn() + Send + Sync>>,
}

impl McpServerRegistry {
    /// Register or replace a server. The caller checks ownership.
    /// Replacement keeps the original registration slot (upstream `Map.set`).
    pub fn register(&mut self, server: RegisteredMcpServer) {
        match self
            .servers
            .iter_mut()
            .find(|entry| entry.name == server.name)
        {
            Some(entry) => *entry = server,
            None => self.servers.push(server),
        }
        if let Some(listener) = &self.change_listener {
            listener();
        }
    }

    /// Remove a server registered by `extensionPath`. Servers of other
    /// extensions are left alone.
    pub fn unregister(&mut self, name: &str, extension_path: &str) {
        let owned = self
            .servers
            .iter()
            .find(|server| server.name == name)
            .map(|server| server.extension_path == extension_path)
            .unwrap_or(false);
        if !owned {
            return;
        }
        self.servers.retain(|server| server.name != name);
        if let Some(listener) = &self.change_listener {
            listener();
        }
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredMcpServer> {
        self.servers.iter().find(|server| server.name == name)
    }

    /// Clones of the registered servers, in registration order.
    pub fn list(&self) -> Vec<RegisteredMcpServer> {
        self.servers.clone()
    }

    /// Called after every change. The runner sets it when it binds, to emit
    /// `mcp_servers_change`.
    pub fn set_change_listener(&mut self, listener: Option<Box<dyn Fn() + Send + Sync>>) {
        self.change_listener = listener;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(pairs: Vec<(&str, Value)>) -> OrderedMap<Value> {
        OrderedMap::from_pairs(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value)),
        )
    }

    #[test]
    fn loopback_redirect_uris() {
        assert!(is_loopback_redirect_uri(
            "http://localhost:8080/oauth/callback"
        ));
        assert!(is_loopback_redirect_uri("http://127.0.0.1:1234/callback"));
        assert!(is_loopback_redirect_uri("http://[::1]:9999/callback"));
        assert!(!is_loopback_redirect_uri("https://localhost/callback"));
        assert!(!is_loopback_redirect_uri("http://example.com/callback"));
        assert!(!is_loopback_redirect_uri("http://localhost/callback?x=1"));
        assert!(!is_loopback_redirect_uri("http://localhost/callback#f"));
        assert!(!is_loopback_redirect_uri("not a url"));
    }

    #[test]
    fn validates_stdio_and_http_entries() {
        let stdio = validate_mcp_server_config(
            "fs",
            &config(vec![
                ("command", json!("npx")),
                ("args", json!(["-y", "server.ts"])),
                ("exposure", json!("codemode-deferred")),
            ]),
        )
        .unwrap();
        assert_eq!(stdio.command(), Some("npx"));
        assert_eq!(stdio.exposure(), McpExposure::Codemode);
        assert_eq!(stdio.server_type(), None);
        assert!(stdio.enabled());
        assert!(stdio.timeout().is_none());

        let http = validate_mcp_server_config(
            "cloud",
            &config(vec![
                ("url", json!("https://example.com/mcp")),
                ("headers", json!({"Authorization": "Bearer t"})),
                ("timeout", json!(30)),
            ]),
        )
        .unwrap();
        assert_eq!(http.url(), Some("https://example.com/mcp"));
        assert_eq!(http.timeout(), Some(30.0));
        assert_eq!(
            http.string_record("headers").unwrap(),
            vec![("Authorization".to_string(), "Bearer t".to_string())]
        );
    }

    #[test]
    fn validation_error_messages_are_upstream_exact() {
        let cases: Vec<(String, OrderedMap<Value>, &str)> = vec![
            (
                "bad name!".to_string(),
                config(vec![]),
                "invalid server name \"bad name!\" (use letters, digits, \"_\" and \"-\")",
            ),
            (
                "s".to_string(),
                config(vec![("exposure", json!("nope"))]),
                "server \"s\": exposure must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\"",
            ),
            (
                "s".to_string(),
                config(vec![("toolExposure", json!(5))]),
                "server \"s\": toolExposure must map tool names to exposures",
            ),
            (
                "s".to_string(),
                config(vec![("toolExposure", json!({"t": "no"}))]),
                "server \"s\": toolExposure \"t\" must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\"",
            ),
            (
                "s".to_string(),
                config(vec![("enabled", json!("yes"))]),
                "server \"s\": enabled must be a boolean",
            ),
            (
                "s".to_string(),
                config(vec![("timeout", json!(0))]),
                "server \"s\": timeout must be a positive number of seconds",
            ),
            (
                "s".to_string(),
                config(vec![("type", json!("sse")), ("command", json!("x"))]),
                "server \"s\": legacy SSE transport is not supported; use the streamable HTTP URL",
            ),
            (
                "s".to_string(),
                config(vec![("url", json!("ftp://x"))]),
                "server \"s\": url must be an http or https URL",
            ),
            (
                "s".to_string(),
                config(vec![("url", json!("https://x")), ("headers", json!(5))]),
                "server \"s\": headers must map names to strings",
            ),
            (
                "s".to_string(),
                config(vec![
                    ("url", json!("https://x")),
                    ("oauth", json!({"callbackPort": 0})),
                ]),
                "server \"s\": oauth.callbackPort must be a port number",
            ),
            (
                "s".to_string(),
                config(vec![
                    ("url", json!("https://x")),
                    ("oauth", json!({"callbackUrl": "https://x/cb"})),
                ]),
                "server \"s\": oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] without query or fragment",
            ),
            (
                "s".to_string(),
                config(vec![("command", json!("x")), ("args", json!([1]))]),
                "server \"s\": args must be an array of strings",
            ),
            (
                "s".to_string(),
                config(vec![("command", json!("x")), ("env", json!({"A": 1}))]),
                "server \"s\": env must map names to strings",
            ),
            (
                "s".to_string(),
                config(vec![]),
                "server \"s\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)",
            ),
        ];
        for (name, raw, expected) in cases {
            assert_eq!(
                validate_mcp_server_config(&name, &raw).unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn tool_exposure_resolution_order_and_patterns() {
        let server = validate_mcp_server_config(
            "s",
            &config(vec![
                ("command", json!("x")),
                ("exposure", json!("direct")),
                (
                    "toolExposure",
                    json!({"se*": "deferred", "secret*": "hidden", "exact": "codemode"}),
                ),
            ]),
        )
        .unwrap();
        // Exact name wins over patterns.
        assert_eq!(
            get_mcp_tool_exposure(&server, "exact"),
            McpExposure::Codemode
        );
        // Among patterns, the FIRST match in the object wins (not the most
        // specific): "se*" matches "secret" first.
        assert_eq!(
            get_mcp_tool_exposure(&server, "secret"),
            McpExposure::Deferred
        );
        assert_eq!(get_mcp_tool_exposure(&server, "other"), McpExposure::Direct);
        // `*` regex metacharacters in the pattern are literal.
        let literal = validate_mcp_server_config(
            "s2",
            &config(vec![
                ("command", json!("x")),
                ("toolExposure", json!({"a.b": "hidden", "a*b": "direct"})),
            ]),
        )
        .unwrap();
        assert_eq!(get_mcp_tool_exposure(&literal, "a.b"), McpExposure::Hidden);
        assert_eq!(get_mcp_tool_exposure(&literal, "axb"), McpExposure::Direct);
        // Default exposure is codemode.
        let plain =
            validate_mcp_server_config("s3", &config(vec![("command", json!("x"))])).unwrap();
        assert_eq!(get_mcp_tool_exposure(&plain, "t"), McpExposure::Codemode);
    }

    #[test]
    fn registry_scopes_unregister_to_the_owner() {
        let mut registry = McpServerRegistry::default();
        let events = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let sink = std::sync::Arc::clone(&events);
        // Box<dyn Fn> is Send+Sync-free here; the listener surface is typed
        // for the multi-threaded runner, but the test drives it directly.
        let listener: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            sink.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        registry.set_change_listener(Some(listener));

        let make = |name: &str, path: &str| RegisteredMcpServer {
            name: name.to_string(),
            config: validate_mcp_server_config(name, &config(vec![("command", json!("x"))]))
                .unwrap(),
            extension_path: path.to_string(),
        };
        registry.register(make("a", "/ext1"));
        registry.register(make("b", "/ext2"));
        assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(registry.list().len(), 2);

        // Wrong owner: no removal, no event.
        registry.unregister("a", "/ext-other");
        assert!(registry.get("a").is_some());
        assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst), 2);
        // Right owner: removed, event fires.
        registry.unregister("a", "/ext1");
        assert!(registry.get("a").is_none());
        assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst), 3);
        // Replacement keeps registration order of the original slot.
        registry.register(make("b", "/ext2"));
        let names: Vec<String> = registry
            .list()
            .iter()
            .map(|server| server.name.clone())
            .collect();
        assert_eq!(names, vec!["b".to_string()]);
    }
}
