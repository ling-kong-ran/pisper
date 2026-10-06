//! OAuth surface of the MCP client, ported from upstream
//! `packages/mcp/src/oauth/` (itself adapted from
//! modelcontextprotocol/typescript-sdk v1.29.0): discovery of protected
//! resource and authorization server metadata ([`discovery`]), the
//! authorization-code + PKCE flow with dynamic client registration
//! ([`flow`]), the default stateful provider ([`provider`]), the loopback
//! redirect receiver ([`callback`]), the protocol error shapes ([`errors`])
//! and the dependency-free structural validators ([`types`]).
//!
//! Mirrors the upstream `@earendil-works/pi-mcp/oauth` entry point's
//! re-exports 1:1.

pub mod callback;
pub mod discovery;
pub mod errors;
pub mod flow;
pub mod provider;
pub mod types;

pub use callback::{
    OAuthCallback, OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions,
};
pub use discovery::{
    build_authorization_server_discovery_urls, discover_authorization_server_metadata,
    discover_oauth_server_info, discover_protected_resource_metadata, parse_www_authenticate,
    resource_url_from_server_url, select_resource, DiscoveryOptions,
};
pub use errors::{
    McpOAuthAuthorizationRequiredError, OAuthError, OAuthFlowError, OAuthInsecureEndpointError,
    OAuthIssuerMismatchError, OAuthRegistrationError,
};
pub use flow::{
    adapt_oauth_provider, authorize_mcp, exchange_authorization_code, refresh_authorization,
    register_client, start_authorization, step_up_scope, AdaptedOAuthProvider,
    AddClientAuthentication, CredentialKind, FormParams, OAuthClientMetadataDocument,
    OAuthClientProvider, OAuthFlowOptions, OAuthFlowResult, TokenRequestOptions,
};
pub use provider::{
    McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore,
    MemoryOAuthStateStore,
};
pub use types::{
    parse_authorization_server_metadata, parse_client_information, parse_oauth_tokens,
    parse_protected_resource_metadata, AuthorizationServerMetadata, OAuthChallenge,
    OAuthClientInformationMixed, OAuthClientMetadata, OAuthDiscoveryState,
    OAuthProtectedResourceMetadata, OAuthServerInfo, OAuthTokens,
};
