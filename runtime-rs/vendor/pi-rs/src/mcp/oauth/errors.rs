//! OAuth error types, ported from upstream
//! `packages/mcp/src/oauth/errors.ts`. Display strings are byte-identical.

use std::fmt;

/// Upstream `OAuthError`: a protocol-level OAuth error (`error`,
/// `error_description`, `error_uri` from the token endpoint).
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthError {
    pub code: String,
    pub message: String,
    pub error_uri: Option<String>,
}

impl OAuthError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        error_uri: Option<String>,
    ) -> Self {
        let code = code.into();
        let message = message.into();
        // Upstream `super(message || code)`.
        let message = if message.is_empty() {
            code.clone()
        } else {
            message
        };
        OAuthError {
            code,
            message,
            error_uri,
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for OAuthError {}

/// Unified OAuth flow error (upstream distinguishes by `instanceof`).
#[derive(Debug, Clone)]
pub enum OAuthFlowError {
    OAuth(OAuthError),
    IssuerMismatch(OAuthIssuerMismatchError),
    InsecureEndpoint(OAuthInsecureEndpointError),
    Registration(OAuthRegistrationError),
    /// `McpOAuthAuthorizationRequiredError`: the flow ended in a redirect, so
    /// the user has to authorize (thrown by `adaptOAuthProvider`).
    AuthorizationRequired(McpOAuthAuthorizationRequiredError),
    /// Upstream `TypeError` from `fetch` — a network-level failure (rethrown
    /// out of `discoverOAuthServerInfo`, retried nowhere).
    Network(String),
    /// Plain `Error` objects (validation, unexpected HTTP failures).
    Other(String),
}

impl OAuthFlowError {
    /// Upstream `error instanceof OAuthError` protocol-error code, when this
    /// is one.
    pub fn oauth_code(&self) -> Option<&str> {
        match self {
            OAuthFlowError::OAuth(error) => Some(&error.code),
            _ => None,
        }
    }

    /// Upstream `error instanceof OAuthInsecureEndpointError`.
    pub fn is_insecure_endpoint(&self) -> bool {
        matches!(self, OAuthFlowError::InsecureEndpoint(_))
    }
}

impl fmt::Display for OAuthFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OAuthFlowError::OAuth(error) => write!(formatter, "{error}"),
            OAuthFlowError::IssuerMismatch(error) => write!(formatter, "{error}"),
            OAuthFlowError::InsecureEndpoint(error) => write!(formatter, "{error}"),
            OAuthFlowError::Registration(error) => write!(formatter, "{error}"),
            OAuthFlowError::AuthorizationRequired(error) => write!(formatter, "{error}"),
            OAuthFlowError::Network(message) => write!(formatter, "{message}"),
            OAuthFlowError::Other(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for OAuthFlowError {}

impl From<OAuthError> for OAuthFlowError {
    fn from(error: OAuthError) -> Self {
        OAuthFlowError::OAuth(error)
    }
}

/// Upstream `OAuthIssuerMismatchError` (v1.0.0: `received` is `None` when an
/// authorization response lacks the `iss` parameter its server promised,
/// RFC 9207).
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthIssuerMismatchError {
    pub expected: String,
    pub received: Option<String>,
}

impl OAuthIssuerMismatchError {
    pub fn new(expected: impl Into<String>, received: Option<String>) -> Self {
        OAuthIssuerMismatchError {
            expected: expected.into(),
            received,
        }
    }
}

impl fmt::Display for OAuthIssuerMismatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let received = match &self.received {
            Some(received) => serde_json::to_string(received).unwrap_or_default(),
            None => "none".to_string(),
        };
        write!(
            formatter,
            "OAuth issuer mismatch: expected {}, received {}",
            serde_json::to_string(&self.expected).unwrap_or_default(),
            received,
        )
    }
}

impl std::error::Error for OAuthIssuerMismatchError {}

/// Upstream `OAuthInsecureEndpointError`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthInsecureEndpointError {
    pub endpoint: String,
}

impl fmt::Display for OAuthInsecureEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Refusing to send OAuth credentials to non-HTTPS endpoint {}",
            self.endpoint
        )
    }
}

impl std::error::Error for OAuthInsecureEndpointError {}

/// Upstream `OAuthRegistrationError`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthRegistrationError {
    pub status: u16,
    pub body: String,
}

impl fmt::Display for OAuthRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OAuth dynamic client registration failed with status {}: {}",
            self.status, self.body
        )
    }
}

impl std::error::Error for OAuthRegistrationError {}

/// Upstream `McpOAuthAuthorizationRequiredError`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpOAuthAuthorizationRequiredError;

impl fmt::Display for McpOAuthAuthorizationRequiredError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MCP OAuth authorization requires user interaction"
        )
    }
}

impl std::error::Error for McpOAuthAuthorizationRequiredError {}
