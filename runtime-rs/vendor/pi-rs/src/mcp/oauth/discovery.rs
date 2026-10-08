//! OAuth server discovery, ported from upstream
//! `packages/mcp/src/oauth/discovery.ts` (itself adapted from
//! modelcontextprotocol/typescript-sdk v1.29.0 `src/client/auth.ts`, minus the
//! Zod/CORS shims and with authorization-server issuer validation).
//!
//! Well-known URLs are built origin-rooted exactly like the upstream
//! `new URL(path, server.origin)` constructions (`Url::join` with a leading
//! `/`), and request headers are spelled as upstream passes them to `fetch`
//! (`Accept`, `MCP-Protocol-Version`), which is what the oracle's recorded
//! request headers pin.

use regex::Regex;
use url::Url;

use crate::mcp::auth_provider::{default_fetch, FetchError, FetchRequest, FetchResponse, McpFetch};
use crate::mcp::oauth::errors::{OAuthFlowError, OAuthIssuerMismatchError};
use crate::mcp::oauth::types::{
    parse_authorization_server_metadata, parse_protected_resource_metadata,
    AuthorizationServerMetadata, OAuthChallenge, OAuthProtectedResourceMetadata, OAuthServerInfo,
};
use crate::mcp::protocol::types::LATEST_PROTOCOL_VERSION;

/// Upstream fetch failure mapped into the flow error surface. Network-level
/// failures stay classifiable through [`OAuthFlowError::Network`] (upstream
/// rethrows `TypeError` out of `discoverOAuthServerInfo`), everything else
/// becomes the plain-`Error` analog.
fn flow_error(error: FetchError) -> OAuthFlowError {
    if error.network {
        OAuthFlowError::Network(error.message)
    } else {
        OAuthFlowError::Other(error.message)
    }
}

/// 4xx and 502 mean "not here", so discovery tries the next candidate URL.
fn is_discovery_miss(status: u16) -> bool {
    (400..500).contains(&status) || status == 502
}

/// Path suffix for `/.well-known/<kind><path>`; empty for the root path.
fn path_suffix(pathname: &str) -> &str {
    pathname.strip_suffix('/').unwrap_or(pathname)
}

/// One `resource_metadata=`/`scope=`/... field of a challenge header, porting
/// the upstream match `(?:^|[,\s])${name}=(?:"([^"]*)"|([^\s,]+))`
/// case-insensitively (quoted value first, then a bare token).
fn field(header: &str, name: &str) -> Option<String> {
    let pattern = format!(
        r#"(?:^|[,\s]){}=(?:"([^"]*)"|([^\s,]+))"#,
        regex::escape(name)
    );
    let regex = Regex::new(&pattern).ok()?;
    let captures = regex.captures(header)?;
    // v1.0.0: an empty value (`scope=""`) carries no information, so it
    // counts as absent (`match?.[1] || match?.[2] || undefined`).
    let quoted = captures
        .get(1)
        .map(|value| value.as_str())
        .filter(|v| !v.is_empty());
    let bare = captures
        .get(2)
        .map(|value| value.as_str())
        .filter(|v| !v.is_empty());
    quoted.or(bare).map(|value| value.to_string())
}

/// Upstream `parseWwwAuthenticate`: only `Bearer`/`DPoP` challenges carry the
/// fields the flow needs; every other scheme parses to an empty challenge.
pub fn parse_www_authenticate(header: Option<&str>) -> OAuthChallenge {
    let Some(header) = header else {
        return OAuthChallenge::default();
    };
    let scheme = header
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if scheme != "bearer" && scheme != "dpop" {
        return OAuthChallenge::default();
    }
    let resource_metadata_url =
        field(header, "resource_metadata").and_then(|value| Url::parse(&value).ok());
    OAuthChallenge {
        resource_metadata_url,
        scope: field(header, "scope"),
        error: field(header, "error"),
        error_description: field(header, "error_description"),
    }
}

async fn fetch_metadata(
    fetch: &McpFetch,
    url: Url,
    protocol_version: &str,
) -> Result<FetchResponse, FetchError> {
    let request = FetchRequest {
        url,
        method: "GET".to_string(),
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            (
                "MCP-Protocol-Version".to_string(),
                protocol_version.to_string(),
            ),
        ],
        body: None,
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    fetch(request).await
}

/// Options shared by the discovery entry points: an injected fetch (upstream
/// `options.fetch`, defaulting to the platform fetch) and the protocol
/// version header value.
#[derive(Clone, Default)]
pub struct DiscoveryOptions {
    pub fetch: Option<McpFetch>,
    pub protocol_version: Option<String>,
}

impl DiscoveryOptions {
    /// Upstream `options.fetch ?? globalThis.fetch`.
    pub fn fetch_or_default(&self) -> McpFetch {
        self.fetch.clone().unwrap_or_else(default_fetch)
    }

    fn version(&self) -> &str {
        self.protocol_version
            .as_deref()
            .unwrap_or(LATEST_PROTOCOL_VERSION)
    }
}

/// Upstream `discoverProtectedResourceMetadata`. `resource_metadata_url` is
/// the challenge-provided direct URL; without one the well-known path under
/// the server origin is probed first, falling back to the root for pathed
/// servers.
pub async fn discover_protected_resource_metadata(
    server_url: &str,
    options: DiscoveryOptions,
    resource_metadata_url: Option<String>,
) -> Result<OAuthProtectedResourceMetadata, OAuthFlowError> {
    let server =
        Url::parse(server_url).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    let fetch = options.fetch_or_default();
    let version = options.version().to_string();
    let mut url = match &resource_metadata_url {
        Some(url) => Url::parse(url).map_err(|error| OAuthFlowError::Other(error.to_string()))?,
        None => server
            .join(&format!(
                "/.well-known/oauth-protected-resource{}",
                path_suffix(server.path())
            ))
            .map_err(|error| OAuthFlowError::Other(error.to_string()))?,
    };
    let mut response = fetch_metadata(&fetch, url.clone(), &version)
        .await
        .map_err(flow_error)?;
    if resource_metadata_url.is_none() && server.path() != "/" && is_discovery_miss(response.status)
    {
        response.discard();
        url = server
            .join("/.well-known/oauth-protected-resource")
            .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
        response = fetch_metadata(&fetch, url, &version)
            .await
            .map_err(flow_error)?;
    }
    if !(200..300).contains(&response.status) {
        let status = response.status;
        response.discard();
        return Err(OAuthFlowError::Other(format!(
            "HTTP {status} loading OAuth protected resource metadata"
        )));
    }
    let text = response.into_text().await.map_err(OAuthFlowError::Other)?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    parse_protected_resource_metadata(value).map_err(OAuthFlowError::Other)
}

/// Upstream `buildAuthorizationServerDiscoveryUrls`: the oauth candidate, the
/// oidc candidate under `/.well-known`, and — for a pathed issuer — the
/// path-first oidc candidate. The second tuple element is the upstream
/// `"oauth" | "oidc"` type tag.
pub fn build_authorization_server_discovery_urls(
    authorization_server_url: &str,
) -> Result<Vec<(Url, &'static str)>, OAuthFlowError> {
    let issuer = Url::parse(authorization_server_url)
        .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    let path = path_suffix(issuer.path()).to_string();
    let mut urls = vec![
        (
            issuer
                .join(&format!("/.well-known/oauth-authorization-server{path}"))
                .map_err(|error| OAuthFlowError::Other(error.to_string()))?,
            "oauth",
        ),
        (
            issuer
                .join(&format!("/.well-known/openid-configuration{path}"))
                .map_err(|error| OAuthFlowError::Other(error.to_string()))?,
            "oidc",
        ),
    ];
    if !path.is_empty() {
        urls.push((
            issuer
                .join(&format!("{path}/.well-known/openid-configuration"))
                .map_err(|error| OAuthFlowError::Other(error.to_string()))?,
            "oidc",
        ));
    }
    Ok(urls)
}

/// Upstream `discoverAuthorizationServerMetadata`: probe the candidate URLs in
/// order, skipping discovery misses, failing hard on other statuses, and —
/// unless skipped — rejecting an issuer that does not match the probed URL.
/// (Upstream's trailing `undefined` return is dead code: the candidate list is
/// never empty; the port mirrors the loop as always returning or erroring.)
pub async fn discover_authorization_server_metadata(
    authorization_server_url: &str,
    options: DiscoveryOptions,
    skip_issuer_validation: bool,
) -> Result<Option<AuthorizationServerMetadata>, OAuthFlowError> {
    let fetch = options.fetch_or_default();
    let version = options.version().to_string();
    for (url, _kind) in build_authorization_server_discovery_urls(authorization_server_url)? {
        let response = fetch_metadata(&fetch, url.clone(), &version)
            .await
            .map_err(flow_error)?;
        if !(200..300).contains(&response.status) {
            let status = response.status;
            response.discard();
            if is_discovery_miss(status) {
                continue;
            }
            return Err(OAuthFlowError::Other(format!(
                "HTTP {status} loading authorization server metadata from {url}"
            )));
        }
        let text = response.into_text().await.map_err(OAuthFlowError::Other)?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
        let metadata = parse_authorization_server_metadata(value).map_err(OAuthFlowError::Other)?;
        if !skip_issuer_validation {
            // URL parsing adds a trailing slash to bare origins, so compare
            // without one on either side.
            let expected = authorization_server_url.to_string();
            if trim_slash(&metadata.issuer()) != trim_slash(&expected) {
                return Err(OAuthFlowError::IssuerMismatch(
                    OAuthIssuerMismatchError::new(expected, Some(metadata.issuer())),
                ));
            }
        }
        return Ok(Some(metadata));
    }
    // Every candidate missed (upstream's trailing `undefined` return).
    Ok(None)
}

/// The upstream `trim` helper: drop exactly one trailing slash.
fn trim_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

/// Upstream `discoverOAuthServerInfo`: protected-resource metadata first (a
/// failed probe only falls back to the origin; a network-level fetch failure
/// rethrows like upstream `TypeError`). The inner discovery calls always use
/// the default protocol version, exactly like the upstream option forwarding.
pub async fn discover_oauth_server_info(
    server_url: &str,
    options: DiscoveryOptions,
    resource_metadata_url: Option<String>,
    authorization_server_metadata_url: Option<String>,
    skip_issuer_validation: bool,
) -> Result<OAuthServerInfo, OAuthFlowError> {
    let inner = DiscoveryOptions {
        fetch: options.fetch.clone(),
        protocol_version: None,
    };
    let mut resource_metadata: Option<OAuthProtectedResourceMetadata> = None;
    match discover_protected_resource_metadata(server_url, inner.clone(), resource_metadata_url)
        .await
    {
        Ok(metadata) => resource_metadata = Some(metadata),
        // Upstream rethrows `TypeError` (network-level fetch failures) and
        // swallows every other error.
        Err(error @ OAuthFlowError::Network(_)) => return Err(error),
        Err(OAuthFlowError::Other(_)) => {}
        Err(error) => return Err(error),
    }
    // v1.0.0: a configured metadata document replaces discovery. It is
    // trusted as configured, so its issuer is not checked.
    if let Some(metadata_url) = &authorization_server_metadata_url {
        let url =
            Url::parse(metadata_url).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
        let fetch = inner.fetch_or_default();
        let response = fetch_metadata(&fetch, url.clone(), LATEST_PROTOCOL_VERSION)
            .await
            .map_err(flow_error)?;
        if !(200..300).contains(&response.status) {
            let status = response.status;
            response.discard();
            return Err(OAuthFlowError::Other(format!(
                "HTTP {status} loading authorization server metadata from {url}"
            )));
        }
        let text = response.into_text().await.map_err(OAuthFlowError::Other)?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
        let metadata = parse_authorization_server_metadata(value).map_err(OAuthFlowError::Other)?;
        return Ok(OAuthServerInfo {
            authorization_server_url: metadata.issuer(),
            authorization_server_metadata: Some(metadata),
            resource_metadata,
        });
    }
    let authorization_server_url = match resource_metadata
        .as_ref()
        .and_then(|metadata| metadata.authorization_servers().into_iter().next())
    {
        Some(server) => server,
        None => {
            let server =
                Url::parse(server_url).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
            // Upstream `String(new URL("/", serverUrl))`: the origin with a
            // trailing slash.
            server
                .join("/")
                .map_err(|error| OAuthFlowError::Other(error.to_string()))?
                .to_string()
        }
    };
    let authorization_server_metadata = discover_authorization_server_metadata(
        &authorization_server_url,
        inner,
        skip_issuer_validation,
    )
    .await?;
    Ok(OAuthServerInfo {
        authorization_server_url,
        authorization_server_metadata,
        resource_metadata,
    })
}

/// Upstream `resourceUrlFromServerUrl`: the URL without its fragment.
pub fn resource_url_from_server_url(value: &str) -> Result<Url, OAuthFlowError> {
    let mut url = Url::parse(value).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    url.set_fragment(None);
    Ok(url)
}

/// Upstream `selectResource`: the protected resource applies when it shares
/// the MCP server's origin and its path is a prefix of the server's.
pub fn select_resource(
    server_url: &str,
    metadata: Option<&OAuthProtectedResourceMetadata>,
) -> Result<Option<String>, OAuthFlowError> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let requested = resource_url_from_server_url(server_url)?;
    let configured = Url::parse(&metadata.resource())
        .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    if requested.origin() != configured.origin() {
        return Err(OAuthFlowError::Other(format!(
            "Protected resource {} does not match MCP server {}",
            metadata.resource(),
            requested
        )));
    }
    let requested_path = ensured_slash(requested.path());
    let configured_path = ensured_slash(configured.path());
    if !requested_path.starts_with(&configured_path) {
        return Err(OAuthFlowError::Other(format!(
            "Protected resource {} does not match MCP server {}",
            metadata.resource(),
            requested
        )));
    }
    Ok(Some(metadata.resource()))
}

/// Upstream normalizes both paths to end with `/` before the prefix check.
fn ensured_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}
