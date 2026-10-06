//! OAuth oracle tests (parsing, discovery, flow, provider, adapted provider,
//! loopback callback server) replaying
//! `tests/fixtures/mcp_oracle/mcp_oracle.json`; see
//! [`crate::mcp::mcp_oracle_tests`] for the provenance and comparison
//! conventions. All HTTP scenarios use injected recorder fetches — no
//! network; the callback server is hit with raw loopback sockets.

use std::sync::Arc;
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use url::Url;

use crate::mcp::auth_provider::{
    ByteStream, FetchRequest, FetchResponse, McpFetch, UnauthorizedContext,
};
use crate::mcp::mcp_oracle_tests::{assert_canonical, oracle, scenario_lock, stringify, FIXED_NOW};
use crate::mcp::oauth::callback::{
    OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions,
};
use crate::mcp::oauth::discovery::{
    build_authorization_server_discovery_urls, discover_authorization_server_metadata,
    discover_oauth_server_info, discover_protected_resource_metadata, parse_www_authenticate,
    select_resource, DiscoveryOptions,
};
use crate::mcp::oauth::errors::{OAuthFlowError, OAuthIssuerMismatchError};
use crate::mcp::oauth::flow::{
    adapt_oauth_provider, authorize_mcp, exchange_authorization_code, refresh_authorization,
    register_client, start_authorization, CredentialKind, FormParams, OAuthClientProvider,
    OAuthFlowOptions, OAuthFlowResult, TokenRequestOptions,
};
use crate::mcp::oauth::provider::{
    McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore,
    MemoryOAuthStateStore,
};
use crate::mcp::oauth::types::{
    parse_authorization_server_metadata, parse_client_information, parse_oauth_tokens,
    parse_protected_resource_metadata, OAuthClientMetadata, OAuthDiscoveryState, OAuthTokens,
};
use crate::mcp::protocol::jsonrpc::McpClientError;

/// The scenario AS metadata used all over the capture.
fn as_metadata() -> Value {
    json!({
        "issuer": "https://as.example",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "response_types_supported": ["code"],
    })
}

fn as_metadata_full() -> Value {
    json!({
        "issuer": "https://as.example",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "registration_endpoint": "https://as.example/register",
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
    })
}

// ---------------------------------------------------------------------------
// oauth_parsing
// ---------------------------------------------------------------------------

#[test]
fn oauth_parsing_matches_the_capture() {
    let expected = &oracle()["oauth_parsing"];

    // WWW-Authenticate challenges.
    for case in expected["wwwAuthenticate"].as_array().unwrap() {
        let header = &case["header"];
        let challenge = parse_www_authenticate(header.as_str());
        let mut actual = serde_json::Map::new();
        if let Some(url) = challenge.resource_metadata_url {
            actual.insert("resourceMetadataUrl".into(), json!(url.to_string()));
        }
        if let Some(scope) = challenge.scope {
            actual.insert("scope".into(), json!(scope));
        }
        if let Some(error) = challenge.error {
            actual.insert("error".into(), json!(error));
        }
        if let Some(description) = challenge.error_description {
            actual.insert("errorDescription".into(), json!(description));
        }
        let expected_fields: serde_json::Map<String, Value> = case
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.as_str() != "header")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_canonical(
            &format!("www-authenticate {header}"),
            Value::Object(actual),
            &Value::Object(expected_fields),
        );
    }

    // Structural validators.
    let protected_resource = parse_protected_resource_metadata(json!({
        "resource": "https://rs.example/mcp",
        "authorization_servers": ["https://as.example"],
        "scopes_supported": ["read", "write"],
        "custom_future_field": { "v": 1 },
    }))
    .expect("protected resource parses");
    assert_canonical(
        "protected resource",
        Value::Object(protected_resource.raw().clone()),
        &expected["protectedResource"],
    );
    let error = parse_protected_resource_metadata(json!({})).expect_err("missing resource");
    assert_eq!(
        error,
        expected["protectedResourceInvalid"].as_str().unwrap()
    );

    let authorization_server = parse_authorization_server_metadata(json!({
        "issuer": "https://as.example/",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "registration_endpoint": "https://as.example/register",
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
        "client_id_metadata_document_supported": true,
        "extra": "kept",
    }))
    .expect("authorization server metadata parses");
    assert_canonical(
        "authorization server",
        Value::Object(authorization_server.raw().clone()),
        &expected["authorizationServer"],
    );
    let error = parse_authorization_server_metadata(json!({ "issuer": "https://x" }))
        .expect_err("missing response types");
    assert_eq!(
        error,
        expected["authorizationServerInvalid"].as_str().unwrap()
    );
    let error = parse_authorization_server_metadata(json!({
        "issuer": "https://x",
        "authorization_endpoint": "javascript:alert(1)",
        "token_endpoint": "https://x/token",
        "response_types_supported": ["code"],
    }))
    .expect_err("unsafe endpoint");
    assert_eq!(
        error,
        expected["authorizationServerUnsafe"].as_str().unwrap()
    );

    let tokens = parse_oauth_tokens(json!({
        "access_token": "at", "token_type": "Bearer", "expires_in": "3600",
        "scope": "a b", "refresh_token": "rt", "id_token": "it",
        "unknown": "dropped",
    }))
    .expect("tokens parse");
    assert_canonical("tokens", tokens.to_value(), &expected["tokens"]);
    let error = parse_oauth_tokens(json!({ "access_token": "at" })).expect_err("missing type");
    assert_eq!(error, expected["tokensInvalid"].as_str().unwrap());
    let error = parse_oauth_tokens(json!({
        "access_token": "at", "token_type": "B", "expires_in": "soon",
    }))
    .expect_err("bad expires");
    assert_eq!(error, expected["tokensBadExpires"].as_str().unwrap());

    let client_information = parse_client_information(json!({
        "client_id": "cid", "client_secret": "sec", "client_id_issued_at": 5,
        "client_secret_expires_at": 10, "redirect_uris": ["https://cb"],
        "client_name": "pi", "application_type": "web",
    }))
    .expect("client information parses");
    assert_canonical(
        "client information",
        Value::Object(client_information.raw().clone()),
        &expected["clientInformation"],
    );
    let no_secret = parse_client_information(json!({ "client_id": "cid" })).expect("parses");
    assert_canonical(
        "client information no secret",
        Value::Object(no_secret.raw().clone()),
        &expected["clientInformationNoSecret"],
    );

    // Discovery URL builders.
    let inputs = [
        "https://as.example",
        "https://as.example/",
        "https://as.example/tenant1",
        "https://as.example/tenant1/",
    ];
    for (index, input) in inputs.iter().enumerate() {
        let urls: Vec<Value> = build_authorization_server_discovery_urls(input)
            .expect("urls build")
            .into_iter()
            .map(|(url, kind)| json!({ "url": url.to_string(), "type": kind }))
            .collect();
        assert_canonical(
            &format!("discovery urls {index}"),
            Value::Array(urls),
            &expected["discoveryUrls"][index],
        );
    }
}

// ---------------------------------------------------------------------------
// oauth_discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth_discovery_matches_the_capture() {
    let expected = &oracle()["oauth_discovery"];
    let resource_metadata = json!({
        "resource": "https://rs.example/mcp",
        "authorization_servers": ["https://as.example"],
    });

    // Protected resource discovery: pathed first, fallback to root.
    {
        let resource_metadata_clone = resource_metadata.clone();
        let handler = move |record: &Value, _index: usize| {
            let pathname = Url::parse(record["url"].as_str().unwrap())
                .unwrap()
                .path()
                .to_string();
            if pathname == "/.well-known/oauth-protected-resource/mcp" {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    404,
                    &[],
                    "",
                ))
            } else if pathname == "/.well-known/oauth-protected-resource" {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    resource_metadata_clone.clone(),
                ))
            } else if pathname == "/.well-known/oauth-authorization-server" {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    as_metadata(),
                ))
            } else {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    404,
                    &[],
                    "",
                ))
            }
        };
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(handler), false);
        let meta = discover_protected_resource_metadata(
            "https://rs.example/mcp",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            None,
        )
        .await
        .expect("discovery");
        assert_canonical(
            "protected resource fallback meta",
            Value::Object(meta.raw().clone()),
            &expected["protectedResourceFallback"]["meta"],
        );
        let urls: Vec<Value> = requests
            .lock()
            .expect("log")
            .iter()
            .map(|request| request["url"].clone())
            .collect();
        assert_canonical(
            "fallback request urls",
            json!(urls),
            &expected["protectedResourceFallback"]["requestUrls"],
        );
        let headers: Vec<Value> = requests
            .lock()
            .expect("log")
            .iter()
            .map(|request| request["headers"].clone())
            .collect();
        assert_canonical(
            "fallback request headers",
            json!(headers),
            &expected["protectedResourceFallback"]["requestHeaders"],
        );
    }

    // Authorization server metadata: first candidate answers.
    {
        let handler = move |record: &Value, _index: usize| {
            let pathname = Url::parse(record["url"].as_str().unwrap())
                .unwrap()
                .path()
                .to_string();
            if pathname == "/.well-known/oauth-authorization-server" {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    as_metadata(),
                ))
            } else {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    404,
                    &[],
                    "",
                ))
            }
        };
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(handler), false);
        let meta = discover_authorization_server_metadata(
            "https://as.example",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            false,
        )
        .await
        .expect("discovery")
        .expect("first candidate answers");
        assert_canonical(
            "as meta",
            Value::Object(meta.raw().clone()),
            &expected["authorizationServerDiscovery"]["meta"],
        );
        let urls: Vec<Value> = requests
            .lock()
            .expect("log")
            .iter()
            .map(|request| request["url"].clone())
            .collect();
        assert_canonical(
            "as request urls",
            json!(urls),
            &expected["authorizationServerDiscovery"]["requestUrls"],
        );
    }

    // Issuer mismatch.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({
                        "issuer": "https://other.example",
                        "authorization_endpoint": "https://as.example/authorize",
                        "token_endpoint": "https://as.example/token",
                        "response_types_supported": ["code"],
                    }),
                ))
            }),
            false,
        );
        let error = discover_authorization_server_metadata(
            "https://as.example",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            false,
        )
        .await
        .expect_err("issuer mismatch");
        let OAuthFlowError::IssuerMismatch(mismatch) = error else {
            panic!("expected issuer mismatch, got {error:?}");
        };
        assert_eq!(
            mismatch,
            OAuthIssuerMismatchError {
                expected: expected["issuerMismatch"]["expected"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                received: expected["issuerMismatch"]["received"]
                    .as_str()
                    .map(str::to_string),
            }
        );
        assert_eq!(
            mismatch.to_string(),
            expected["issuerMismatch"]["message"].as_str().unwrap()
        );
    }

    // Metadata HTTP error (non-miss status).
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    503,
                    &[],
                    "busy",
                ))
            }),
            false,
        );
        let error = discover_authorization_server_metadata(
            "https://as.example",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            false,
        )
        .await
        .expect_err("503");
        assert_eq!(
            error.to_string(),
            expected["metadataHttpError"].as_str().unwrap()
        );
    }

    // skipIssuerValidation.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({
                        "issuer": "https://other.example",
                        "authorization_endpoint": "https://as.example/authorize",
                        "token_endpoint": "https://as.example/token",
                        "response_types_supported": ["code"],
                    }),
                ))
            }),
            false,
        );
        let meta = discover_authorization_server_metadata(
            "https://as.example",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            true,
        )
        .await
        .expect("skip validation")
        .expect("candidate answers");
        assert_eq!(
            meta.issuer(),
            expected["skipIssuerValidation"].as_str().unwrap()
        );
    }

    // discoverOAuthServerInfo: no resource metadata -> origin fallback.
    {
        let handler = move |record: &Value, _index: usize| {
            let url = Url::parse(record["url"].as_str().unwrap()).unwrap();
            if url.host_str() == Some("as.example")
                && url.path() == "/.well-known/oauth-authorization-server"
            {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    as_metadata(),
                ))
            } else {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    404,
                    &[],
                    "",
                ))
            }
        };
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(handler), false);
        let info = discover_oauth_server_info(
            "https://rs.example/mcp",
            DiscoveryOptions {
                fetch: Some(fetch),
                protocol_version: None,
            },
            None,
            None,
            false,
        )
        .await
        .expect("server info");
        let actual = json!({
            "authorizationServerUrl": info.authorization_server_url,
            "hasResourceMetadata": info.resource_metadata.is_some(),
            "hasAuthorizationServerMetadata": info.authorization_server_metadata.is_some(),
        });
        for key in [
            "authorizationServerUrl",
            "hasResourceMetadata",
            "hasAuthorizationServerMetadata",
        ] {
            assert_eq!(actual[key], expected["serverInfo"][key], "{key}");
        }
        let urls: Vec<Value> = requests
            .lock()
            .expect("log")
            .iter()
            .map(|request| request["url"].clone())
            .collect();
        assert_canonical(
            "info request urls",
            json!(urls),
            &expected["serverInfo"]["requestUrls"],
        );
    }

    // selectResource: ok, prefix ok, fragment ok, origin mismatch, path
    // mismatch, host case folding (the capture's six cases in order).
    let select_cases: [(&str, bool); 6] = [
        ("https://rs.example/mcp", true),
        ("https://rs.example/mcp/sub/path", true),
        ("https://rs.example/mcp#frag", true),
        ("https://rs.example/other", false),
        ("https://rs.example/mopot", false),
        ("https://RS.example/mcp", true),
    ];
    let parsed =
        parse_protected_resource_metadata(json!({ "resource": "https://rs.example/mcp" })).unwrap();
    for (index, (server_url, ok)) in select_cases.into_iter().enumerate() {
        let case = &expected["selectResource"][index];
        let result = select_resource(server_url, Some(&parsed));
        match (result, ok) {
            (Ok(resource), true) => {
                assert_eq!(
                    resource.as_deref(),
                    Some(case["resource"].as_str().unwrap())
                );
            }
            (Err(error), false) => {
                assert_eq!(error.to_string(), case["error"].as_str().unwrap());
            }
            (other, expected_ok) => {
                panic!("case {index}: got {other:?}, expected ok={expected_ok}")
            }
        }
    }
    assert_eq!(select_resource("https://x", None).unwrap(), None);
}

// ---------------------------------------------------------------------------
// oauth_start_authorization (deterministic PKCE)
// ---------------------------------------------------------------------------

fn challenge_of(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    crate::ai::auth::oauth::pkce::base64url_encode(&Sha256::digest(verifier.as_bytes()))
}

#[tokio::test]
async fn oauth_start_authorization_matches_the_capture() {
    let _guard = scenario_lock().await;
    crate::mcp::test_rng::reset();
    let expected = &oracle()["oauth_start_authorization"];
    let client_information = client_information_from(json!({ "client_id": "client-123" }));

    // No metadata -> /authorize under the server; full parameter order pinned.
    {
        let (url, verifier) = start_authorization(
            "https://as.example",
            None,
            &client_information,
            "http://127.0.0.1:9911/callback",
            Some("read write"),
            Some("st4te"),
            Some("https://rs.example/mcp"),
        )
        .await
        .expect("authorization url");
        assert_eq!(
            url.to_string(),
            expected["noMetadata"]["url"].as_str().unwrap()
        );
        assert_eq!(
            verifier,
            expected["noMetadata"]["verifier"].as_str().unwrap()
        );
        // The challenge in the pinned URL is the real S256 of the verifier.
        let expected_query_challenge = "code_challenge=DwBzhbb51LfusnSGBa_hqYSgo7-j8BTQnip4TOnlzRo";
        assert!(url.to_string().contains(expected_query_challenge));
        assert_eq!(
            challenge_of(&verifier),
            "DwBzhbb51LfusnSGBa_hqYSgo7-j8BTQnip4TOnlzRo"
        );
    }
    // offline_access adds prompt=consent after scope.
    {
        let (url, verifier) = start_authorization(
            "https://as.example",
            None,
            &client_information,
            "http://localhost:9911/callback",
            Some("offline_access read"),
            None,
            None,
        )
        .await
        .expect("authorization url");
        assert_eq!(
            url.to_string(),
            expected["offlineAccess"]["url"].as_str().unwrap()
        );
        assert_eq!(
            verifier,
            expected["offlineAccess"]["verifier"].as_str().unwrap()
        );
    }
    // Metadata endpoints win; metadata without code response types errors.
    {
        let metadata = parse_authorization_server_metadata(json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/authz",
            "token_endpoint": "https://as.example/token",
            "response_types_supported": ["code", "code id_token"],
            "code_challenge_methods_supported": ["S256", "plain"],
        }))
        .expect("metadata");
        let (url, verifier) = start_authorization(
            "https://as.example",
            Some(&metadata),
            &client_information,
            "http://127.0.0.1:9911/callback",
            None,
            None,
            None,
        )
        .await
        .expect("authorization url");
        assert_eq!(
            url.to_string(),
            expected["withMetadata"]["url"].as_str().unwrap()
        );
        assert_eq!(
            verifier,
            expected["withMetadata"]["verifier"].as_str().unwrap()
        );

        let no_code = parse_authorization_server_metadata(json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/authz",
            "token_endpoint": "https://as.example/token",
            "response_types_supported": ["implicit"],
        }))
        .expect("metadata");
        let error = start_authorization(
            "https://as.example",
            Some(&no_code),
            &client_information,
            "http://127.0.0.1:9911/callback",
            None,
            None,
            None,
        )
        .await
        .expect_err("no code response types");
        assert_eq!(
            error.to_string(),
            expected["noCodeResponseTypes"].as_str().unwrap()
        );

        let no_s256 = parse_authorization_server_metadata(json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/authz",
            "token_endpoint": "https://as.example/token",
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["plain"],
        }))
        .expect("metadata");
        let error = start_authorization(
            "https://as.example",
            Some(&no_s256),
            &client_information,
            "http://127.0.0.1:9911/callback",
            None,
            None,
            None,
        )
        .await
        .expect_err("no S256");
        assert_eq!(error.to_string(), expected["noS256"].as_str().unwrap());
    }
}

// ---------------------------------------------------------------------------
// oauth_token_requests
// ---------------------------------------------------------------------------

fn client_information_from(raw: Value) -> crate::mcp::oauth::types::OAuthClientInformationMixed {
    crate::mcp::oauth::types::OAuthClientInformationMixed::from_raw(
        raw.as_object().unwrap().clone(),
    )
}

fn metadata_from(raw: Value) -> crate::mcp::oauth::types::AuthorizationServerMetadata {
    parse_authorization_server_metadata(raw).expect("metadata parses")
}

#[tokio::test]
async fn oauth_token_requests_match_the_capture() {
    let expected = &oracle()["oauth_token_requests"];
    let metadata = metadata_from(json!({
        "issuer": "https://as.example",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "response_types_supported": ["code"],
        "token_endpoint_auth_methods_supported": ["none"],
    }));
    let client_information = client_information_from(json!({ "client_id": "cid-1" }));

    // Authorization code exchange, no secret, with resource.
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "access_token": "at", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "rt" }),
                ))
            }),
            false,
        );
        let tokens = exchange_authorization_code(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information.clone()),
                resource: Some("https://rs.example/mcp".to_string()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "abc",
            "ver-123",
            "http://127.0.0.1:9911/callback",
        )
        .await
        .expect("exchange");
        assert_canonical(
            "exchange tokens",
            tokens.to_value(),
            &expected["exchange"]["tokens"],
        );
        assert_canonical(
            "exchange request",
            requests.lock().expect("log")[0].clone(),
            &expected["exchange"]["request"],
        );
    }
    // Refresh: result merges old refresh_token when absent in the response.
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "access_token": "at2", "token_type": "Bearer" }),
                ))
            }),
            false,
        );
        let tokens = refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information.clone()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "old-rt",
        )
        .await
        .expect("refresh");
        assert_canonical(
            "refresh keep",
            tokens.to_value(),
            &expected["refreshKeep"]["tokens"],
        );
        assert_canonical(
            "refresh keep request",
            requests.lock().expect("log")[0].clone(),
            &expected["refreshKeep"]["request"],
        );
    }
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "access_token": "at3", "token_type": "Bearer", "refresh_token": "new-rt" }),
                ))
            }),
            false,
        );
        let tokens = refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information.clone()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "old-rt",
        )
        .await
        .expect("refresh");
        assert_canonical(
            "refresh replace",
            tokens.to_value(),
            &expected["refreshReplace"]["tokens"],
        );
        assert_canonical(
            "refresh replace request",
            requests.lock().expect("log")[0].clone(),
            &expected["refreshReplace"]["request"],
        );
    }
    // OAuth error body wins over status.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    400,
                    &[("content-type", "application/json")],
                    json!({
                        "error": "invalid_grant",
                        "error_description": "code expired",
                        "error_uri": "https://as.example/err",
                    }),
                ))
            }),
            false,
        );
        let error = exchange_authorization_code(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information.clone()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "x",
            "v",
            "http://cb",
        )
        .await
        .expect_err("oauth error");
        let OAuthFlowError::OAuth(oauth_error) = error else {
            panic!("expected OAuthError, got {error:?}");
        };
        assert_eq!(
            oauth_error.code,
            expected["oauthError"]["code"].as_str().unwrap()
        );
        assert_eq!(
            oauth_error.message,
            expected["oauthError"]["message"].as_str().unwrap()
        );
        assert_eq!(
            oauth_error.error_uri,
            Some(
                expected["oauthError"]["errorUri"]
                    .as_str()
                    .unwrap()
                    .to_string()
            )
        );
    }
    // Non-JSON body with 500 -> server_error with the HTTP text.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    500,
                    &[],
                    "<html>oops</html>",
                ))
            }),
            false,
        );
        let error = exchange_authorization_code(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information.clone()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "x",
            "v",
            "http://cb",
        )
        .await
        .expect_err("server error");
        let OAuthFlowError::OAuth(oauth_error) = error else {
            panic!("expected OAuthError, got {error:?}");
        };
        assert_eq!(
            oauth_error.code,
            expected["serverError"]["code"].as_str().unwrap()
        );
        assert_eq!(
            oauth_error.message,
            expected["serverError"]["message"].as_str().unwrap()
        );
    }
    // Insecure endpoint refused.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[],
                    json!({ "access_token": "a", "token_type": "b" }),
                ))
            }),
            false,
        );
        let insecure_metadata = metadata_from(json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/authorize",
            "token_endpoint": "http://as.example/token",
            "response_types_supported": ["code"],
        }));
        let error = exchange_authorization_code(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(insecure_metadata),
                client_information: Some(client_information.clone()),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "x",
            "v",
            "http://cb",
        )
        .await
        .expect_err("insecure endpoint");
        assert_eq!(
            error.to_string(),
            expected["insecure"]["message"].as_str().unwrap()
        );
    }
    // Loopback http is allowed.
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "access_token": "a", "token_type": "b" }),
                ))
            }),
            false,
        );
        exchange_authorization_code(
            "http://127.0.0.1:8080",
            &TokenRequestOptions {
                client_information: Some(client_information_from(json!({ "client_id": "cid-1" }))),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "x",
            "v",
            "http://127.0.0.1:9911/callback",
        )
        .await
        .expect("loopback");
        assert_eq!(
            requests.lock().expect("log")[0]["url"],
            expected["loopbackTokenUrl"],
        );
    }
}

// ---------------------------------------------------------------------------
// oauth_client_auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth_client_auth_matches_the_capture() {
    let expected = &oracle()["oauth_client_auth"];
    let metadata = metadata_from(json!({
        "issuer": "https://as.example",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "response_types_supported": ["code"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "none"],
    }));
    let token_body = |_record: &Value, _index: usize| {
        Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
            200,
            &[],
            json!({ "access_token": "a", "token_type": "b" }),
        ))
    };

    // client_secret_basic preferred when supported.
    {
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(token_body), false);
        refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata_from(json!({
                    "issuer": "https://as.example",
                    "authorization_endpoint": "https://as.example/authorize",
                    "token_endpoint": "https://as.example/token",
                    "response_types_supported": ["code"],
                    "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
                }))),
                client_information: Some(client_information_from(json!({
                    "client_id": "my-id", "client_secret": "my-secret",
                }))),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "r",
        )
        .await
        .expect("refresh");
        assert_eq!(
            requests.lock().expect("log")[0]["headers"]["authorization"],
            expected["basic"]["authorization"],
        );
        assert_eq!(
            requests.lock().expect("log")[0]["body"],
            expected["basic"]["body"]
        );
    }
    // client_secret_post.
    {
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(token_body), false);
        refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information_from(json!({
                    "client_id": "my-id", "client_secret": "my-secret",
                }))),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "r",
        )
        .await
        .expect("refresh");
        assert_eq!(
            requests.lock().expect("log")[0]["body"],
            expected["post"]["body"]
        );
    }
    // none with secret still available: hinted method wins.
    {
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(token_body), false);
        refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information_from(json!({
                    "client_id": "my-id", "client_secret": "my-secret",
                    "token_endpoint_auth_method": "none",
                }))),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "r",
        )
        .await
        .expect("refresh");
        assert_eq!(
            requests.lock().expect("log")[0]["body"],
            expected["hintedNone"]["body"]
        );
    }
    // No supported list, no secret -> none (client_id in body only).
    {
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(token_body), false);
        refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata_from(json!({
                    "issuer": "https://as.example",
                    "authorization_endpoint": "https://as.example/authorize",
                    "token_endpoint": "https://as.example/token",
                    "response_types_supported": ["code"],
                }))),
                client_information: Some(client_information_from(json!({ "client_id": "my-id" }))),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "r",
        )
        .await
        .expect("refresh");
        assert_eq!(
            requests.lock().expect("log")[0]["body"],
            expected["noSecretNone"]["body"]
        );
    }
    // addClientAuthentication override.
    {
        let (fetch, requests) =
            crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(token_body), false);
        let custom: crate::mcp::oauth::flow::AddClientAuthentication = Arc::new(
            |headers: &mut Vec<(String, String)>,
             params: &mut FormParams,
             url: &Url,
             meta: Option<&crate::mcp::oauth::types::AuthorizationServerMetadata>| {
                Box::pin(async move {
                    crate::mcp::oauth::flow::set_header(headers, "x-custom", "yes");
                    params.set("client_id", "override-id");
                    params.set(
                        "extra",
                        format!("{}{}", url, if meta.is_some() { " meta" } else { "" }),
                    );
                    Ok(())
                })
            },
        );
        refresh_authorization(
            "https://as.example",
            &TokenRequestOptions {
                metadata: Some(metadata.clone()),
                client_information: Some(client_information_from(json!({ "client_id": "my-id" }))),
                add_client_authentication: Some(custom),
                fetch: Some(fetch),
                ..TokenRequestOptions::default()
            },
            "r",
        )
        .await
        .expect("refresh");
        assert_canonical(
            "custom auth headers",
            requests.lock().expect("log")[0]["headers"].clone(),
            &expected["customAuth"]["headers"],
        );
        assert_eq!(
            requests.lock().expect("log")[0]["body"],
            expected["customAuth"]["body"]
        );
    }
}

// ---------------------------------------------------------------------------
// oauth_register_client
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth_register_client_matches_the_capture() {
    let expected = &oracle()["oauth_register_client"];
    let metadata = metadata_from(json!({
        "issuer": "https://as.example",
        "authorization_endpoint": "https://as.example/authorize",
        "token_endpoint": "https://as.example/token",
        "registration_endpoint": "https://as.example/register",
        "response_types_supported": ["code"],
    }));
    let client_metadata = client_metadata_from(json!({
        "client_name": "pi",
        "redirect_uris": ["http://127.0.0.1:9911/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    }));
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({
                        "client_id": "reg-1", "client_secret": "s3cret",
                        "client_id_issued_at": 1,
                        "redirect_uris": ["http://127.0.0.1:9911/callback"],
                        "client_name": "pi",
                    }),
                ))
            }),
            false,
        );
        let info = register_client(
            "https://as.example",
            Some(&metadata),
            &client_metadata,
            Some("read write"),
            Some(fetch),
        )
        .await
        .expect("register");
        assert_canonical(
            "registered",
            Value::Object(info.raw().clone()),
            &expected["registered"]["info"],
        );
        assert_canonical(
            "register request",
            requests.lock().expect("log")[0].clone(),
            &expected["registered"]["request"],
        );
    }
    // No metadata -> /register under the server.
    {
        let (fetch, requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "client_id": "reg-2" }),
                ))
            }),
            false,
        );
        register_client(
            "https://as.example",
            None,
            &client_metadata,
            None,
            Some(fetch),
        )
        .await
        .expect("register");
        assert_eq!(
            requests.lock().expect("log")[0]["url"],
            expected["noMetadataUrl"],
        );
    }
    // Metadata without a registration endpoint errors.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[],
                    json!({ "client_id": "x" }),
                ))
            }),
            false,
        );
        let error = register_client(
            "https://as.example",
            Some(&metadata_from(json!({
                "issuer": "https://as.example",
                "authorization_endpoint": "https://as.example/authorize",
                "token_endpoint": "https://as.example/token",
                "response_types_supported": ["code"],
            }))),
            &client_metadata,
            None,
            Some(fetch),
        )
        .await
        .expect_err("no registration endpoint");
        assert_eq!(
            error.to_string(),
            expected["noRegistrationEndpoint"].as_str().unwrap()
        );
    }
    // HTTP failure -> OAuthRegistrationError with body.
    {
        let (fetch, _requests) = crate::mcp::mcp_transports_oracle_tests::recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(crate::mcp::mcp_transports_oracle_tests::answer(
                    400,
                    &[],
                    "{\"error\":\"invalid_redirect_uri\"}",
                ))
            }),
            false,
        );
        let error = register_client(
            "https://as.example",
            Some(&metadata),
            &client_metadata,
            None,
            Some(fetch),
        )
        .await
        .expect_err("registration error");
        let OAuthFlowError::Registration(registration) = error else {
            panic!("expected OAuthRegistrationError");
        };
        assert_eq!(
            registration.status,
            expected["registrationError"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(
            registration.body,
            expected["registrationError"]["body"].as_str().unwrap()
        );
        assert_eq!(
            registration.to_string(),
            expected["registrationError"]["message"].as_str().unwrap()
        );
    }
}

fn client_metadata_from(raw: Value) -> OAuthClientMetadata {
    OAuthClientMetadata::from_raw(raw.as_object().unwrap().clone())
}

// ---------------------------------------------------------------------------
// oauth_flow (the full authorizeMcp state machine over a stub provider)
// ---------------------------------------------------------------------------

/// The capture's `StubProvider`.
struct StubProvider {
    inner: Mutex<StubState>,
}

#[derive(Default)]
struct StubState {
    client: Option<crate::mcp::oauth::types::OAuthClientInformationMixed>,
    token_set: Option<OAuthTokens>,
    verifier: Option<String>,
    discovery: Option<OAuthDiscoveryState>,
    authorization_url: Option<Url>,
    invalidations: Vec<String>,
}

fn stub_provider() -> Arc<StubProvider> {
    Arc::new(StubProvider {
        inner: Mutex::new(StubState {
            verifier: None,
            discovery: None,
            client: None,
            token_set: None,
            authorization_url: None,
            invalidations: Vec::new(),
        }),
    })
}

impl OAuthClientProvider for StubProvider {
    fn redirect_url(&self) -> String {
        "http://localhost:9911/callback".to_string()
    }

    fn client_metadata(&self) -> OAuthClientMetadata {
        client_metadata_from(json!({
            "client_name": "pi-mcp-test",
            "redirect_uris": ["http://localhost:9911/callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
    }

    fn state(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>> {
        Box::pin(async { Ok("expected-state".to_string()) })
    }

    fn client_information(
        &self,
    ) -> BoxFuture<'_, Option<crate::mcp::oauth::types::OAuthClientInformationMixed>> {
        Box::pin(async move { self.inner.lock().expect("stub").client.clone() })
    }

    fn saves_client_information(&self) -> bool {
        true
    }

    fn save_client_information(
        &self,
        information: crate::mcp::oauth::types::OAuthClientInformationMixed,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            self.inner.lock().expect("stub").client = Some(information);
            Ok(())
        })
    }

    fn tokens(&self) -> BoxFuture<'_, Option<OAuthTokens>> {
        Box::pin(async move { self.inner.lock().expect("stub").token_set.clone() })
    }

    fn save_tokens(&self, tokens: OAuthTokens) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            self.inner.lock().expect("stub").token_set = Some(tokens);
            Ok(())
        })
    }

    fn redirect_to_authorization(&self, url: Url) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            self.inner.lock().expect("stub").authorization_url = Some(url);
            Ok(())
        })
    }

    fn save_code_verifier(&self, verifier: String) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            self.inner.lock().expect("stub").verifier = Some(verifier);
            Ok(())
        })
    }

    fn code_verifier(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>> {
        Box::pin(async move {
            self.inner
                .lock()
                .expect("stub")
                .verifier
                .clone()
                .ok_or_else(|| OAuthFlowError::Other("Missing code verifier".to_string()))
        })
    }

    fn invalidate_credentials(
        &self,
        kind: CredentialKind,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            let mut state = self.inner.lock().expect("stub");
            state.invalidations.push(kind.as_str().to_string());
            if kind == CredentialKind::All || kind == CredentialKind::Client {
                state.client = None;
            }
            if kind == CredentialKind::All || kind == CredentialKind::Tokens {
                state.token_set = None;
            }
            if kind == CredentialKind::All || kind == CredentialKind::Verifier {
                state.verifier = None;
            }
            if kind == CredentialKind::All || kind == CredentialKind::Discovery {
                state.discovery = None;
            }
            Ok(())
        })
    }

    fn save_discovery_state(
        &self,
        state: OAuthDiscoveryState,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move {
            self.inner.lock().expect("stub").discovery = Some(state);
            Ok(())
        })
    }

    fn discovery_state(&self) -> BoxFuture<'_, Option<OAuthDiscoveryState>> {
        Box::pin(async move { self.inner.lock().expect("stub").discovery.clone() })
    }
}

#[tokio::test]
async fn oauth_flow_matches_the_capture() {
    let _guard = scenario_lock().await;
    crate::mcp::test_rng::reset();
    let expected = &oracle()["oauth_flow"];
    let as_metadata_full = as_metadata_full();
    let flow_handler = move |record: &Value, _index: usize| {
        let url = Url::parse(record["url"].as_str().unwrap()).unwrap();
        Ok(match url.path() {
            "/.well-known/oauth-protected-resource/mcp" => {
                crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({
                        "resource": "https://rs.example/mcp",
                        "authorization_servers": ["https://as.example"],
                        "scopes_supported": ["org:read"],
                    }),
                )
            }
            "/.well-known/oauth-authorization-server" => {
                crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    as_metadata_full.clone(),
                )
            }
            "/register" => crate::mcp::mcp_transports_oracle_tests::answer_json(
                200,
                &[("content-type", "application/json")],
                json!({
                    "client_id": "dynamic-client", "client_secret": "dynamic-secret",
                    "client_id_issued_at": 1,
                    "redirect_uris": ["http://localhost:9911/callback"],
                    "client_name": "pi-mcp-test", "scope": "org:read",
                }),
            ),
            "/token" => {
                let params: Vec<(String, String)> = record["body"]
                    .as_str()
                    .map(crate::ai::auth::oauth::parse_urlencoded_pairs)
                    .unwrap_or_default();
                let pair = |name: &str| {
                    params
                        .iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.clone())
                        .unwrap_or_default()
                };
                if pair("grant_type") == "authorization_code" && pair("code") == "good-code" {
                    crate::mcp::mcp_transports_oracle_tests::answer_json(
                        200,
                        &[("content-type", "application/json")],
                        json!({
                            "access_token": "access-1", "token_type": "Bearer",
                            "expires_in": 3600, "refresh_token": "refresh-1",
                            "scope": "org:read",
                        }),
                    )
                } else if pair("grant_type") == "refresh_token"
                    && pair("refresh_token") == "refresh-1"
                {
                    crate::mcp::mcp_transports_oracle_tests::answer_json(
                        200,
                        &[("content-type", "application/json")],
                        json!({ "access_token": "access-2", "token_type": "Bearer", "expires_in": 3600 }),
                    )
                } else if pair("grant_type") == "refresh_token"
                    && pair("refresh_token") == "expired-rt"
                {
                    crate::mcp::mcp_transports_oracle_tests::answer_json(
                        400,
                        &[("content-type", "application/json")],
                        json!({ "error": "invalid_client", "error_description": "client revoked" }),
                    )
                } else {
                    crate::mcp::mcp_transports_oracle_tests::answer_json(
                        400,
                        &[("content-type", "application/json")],
                        json!({ "error": "invalid_grant", "error_description": "code expired" }),
                    )
                }
            }
            _ => crate::mcp::mcp_transports_oracle_tests::answer(404, &[], ""),
        })
    };
    let (flow_fetch, flow_requests) =
        crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(flow_handler), false);
    let options = || OAuthFlowOptions {
        server_url: "https://rs.example/mcp".to_string(),
        fetch: Some(Arc::clone(&flow_fetch)),
        ..OAuthFlowOptions::default()
    };

    // (1) Fresh provider: discovery + registration + REDIRECT.
    let provider = stub_provider();
    let result = authorize_mcp(provider.as_ref(), options())
        .await
        .expect("first flow");
    assert_eq!(result, OAuthFlowResult::Redirect);
    {
        let state = provider.inner.lock().expect("stub");
        assert_eq!(
            state.authorization_url.as_ref().expect("url").to_string(),
            expected["firstAuthorizationUrl"].as_str().unwrap()
        );
        assert_eq!(
            state.verifier.as_deref(),
            Some(expected["firstVerifier"].as_str().unwrap())
        );
        let discovery = state.discovery.as_ref().expect("discovery");
        let actual = json!({
            "authorizationServerUrl": discovery.authorization_server_url,
            "resourceMetadata": discovery
                .resource_metadata
                .as_ref()
                .map(|metadata| Value::Object(metadata.raw().clone())),
        });
        assert_canonical("first discovery", actual, &expected["firstDiscovery"]);
        assert_canonical(
            "first saved client",
            Value::Object(state.client.as_ref().expect("client").raw().clone()),
            &expected["firstSavedClient"],
        );
    }
    let register_request = flow_requests
        .lock()
        .expect("log")
        .iter()
        .find(|request| request["url"].as_str().unwrap_or("").ends_with("/register"))
        .cloned()
        .expect("register request");
    assert_canonical(
        "register request",
        register_request,
        &expected["registerRequest"],
    );

    // (2) Authorization code exchange.
    provider.inner.lock().expect("stub").authorization_url = None;
    let code_options = OAuthFlowOptions {
        authorization_code: Some("good-code".to_string()),
        ..options()
    };
    let result = authorize_mcp(provider.as_ref(), code_options)
        .await
        .expect("code exchange");
    assert_eq!(result, OAuthFlowResult::Authorized);
    {
        let state = provider.inner.lock().expect("stub");
        assert_canonical(
            "tokens after code",
            state.token_set.as_ref().expect("tokens").to_value(),
            &expected["tokensAfterCode"],
        );
    }
    let code_exchange_request = flow_requests
        .lock()
        .expect("log")
        .iter()
        .find(|request| {
            request["url"].as_str().unwrap_or("").ends_with("/token")
                && request["body"]
                    .as_str()
                    .unwrap_or("")
                    .contains("authorization_code")
        })
        .cloned()
        .expect("code exchange request");
    assert_canonical(
        "code exchange request",
        code_exchange_request,
        &expected["codeExchangeRequest"],
    );

    // (3) Stored refresh token: silent refresh -> AUTHORIZED, no redirect.
    provider.inner.lock().expect("stub").authorization_url = None;
    let result = authorize_mcp(provider.as_ref(), options())
        .await
        .expect("refresh");
    assert_eq!(result, OAuthFlowResult::Authorized);
    {
        let state = provider.inner.lock().expect("stub");
        assert_canonical(
            "tokens after refresh",
            state.token_set.as_ref().expect("tokens").to_value(),
            &expected["tokensAfterRefresh"],
        );
        assert!(
            state.authorization_url.is_none(),
            "refresh must not redirect"
        );
    }
    let refresh_request = flow_requests
        .lock()
        .expect("log")
        .iter()
        .find(|request| {
            request["body"]
                .as_str()
                .unwrap_or("")
                .starts_with("grant_type=refresh_token")
        })
        .cloned()
        .expect("refresh request");
    assert_canonical(
        "refresh request",
        refresh_request,
        &expected["refreshRequest"],
    );

    // (4) invalid_client on refresh invalidates everything and re-runs.
    {
        let mut state = provider.inner.lock().expect("stub");
        state.token_set = Some(OAuthTokens {
            access_token: "a".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: Some("expired-rt".to_string()),
            id_token: None,
        });
    }
    assert!(provider
        .inner
        .lock()
        .expect("stub")
        .invalidations
        .is_empty());
    let result = authorize_mcp(provider.as_ref(), options())
        .await
        .expect("invalid client flow");
    assert_eq!(result, OAuthFlowResult::Redirect);
    assert_eq!(
        provider.inner.lock().expect("stub").invalidations,
        vec!["all".to_string()]
    );
    assert!(provider
        .inner
        .lock()
        .expect("stub")
        .authorization_url
        .is_some());

    // (5) invalid_grant invalidates tokens only, then re-runs (redirect).
    {
        let mut state = provider.inner.lock().expect("stub");
        state.invalidations.clear();
        state.token_set = Some(OAuthTokens {
            access_token: "a".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: Some("stale-rt".to_string()),
            id_token: None,
        });
        state.client = Some(client_information_from(json!({
            "client_id": "dynamic-client", "client_secret": "dynamic-secret",
        })));
        state.discovery = Some(OAuthDiscoveryState {
            authorization_server_url: "https://as.example".to_string(),
            authorization_server_metadata: None,
            resource_metadata: None,
            resource_metadata_url: None,
        });
    }
    let stale_handler = move |record: &Value, _index: usize| {
        let url = Url::parse(record["url"].as_str().unwrap()).unwrap();
        Ok(match url.path() {
            "/token" => crate::mcp::mcp_transports_oracle_tests::answer_json(
                400,
                &[("content-type", "application/json")],
                json!({ "error": "invalid_grant", "error_description": "rotated" }),
            ),
            "/.well-known/oauth-authorization-server" => {
                crate::mcp::mcp_transports_oracle_tests::answer_json(
                    200,
                    &[("content-type", "application/json")],
                    as_metadata(),
                )
            }
            _ => crate::mcp::mcp_transports_oracle_tests::answer(404, &[], ""),
        })
    };
    let (flow_fetch2, flow_requests2) =
        crate::mcp::mcp_transports_oracle_tests::recorder_fetch(Arc::new(stale_handler), false);
    let stale_options = OAuthFlowOptions {
        server_url: "https://rs.example/mcp".to_string(),
        fetch: Some(Arc::clone(&flow_fetch2)),
        ..OAuthFlowOptions::default()
    };
    let result = authorize_mcp(provider.as_ref(), stale_options)
        .await
        .expect("invalid grant flow");
    assert_eq!(result, OAuthFlowResult::Redirect);
    assert_eq!(
        provider.inner.lock().expect("stub").invalidations,
        vec!["tokens".to_string()]
    );
    let fifth_requests: Vec<Value> = flow_requests2
        .lock()
        .expect("log")
        .clone()
        .iter()
        .map(|request| {
            let mut subset = serde_json::Map::new();
            subset.insert("url".into(), request["url"].clone());
            if let Some(body) = request.get("body") {
                subset.insert("body".into(), body.clone());
            }
            Value::Object(subset)
        })
        .collect();
    let expected_fifth: Vec<Value> = expected["fifthRequests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| {
            let mut subset = serde_json::Map::new();
            subset.insert("url".into(), request["url"].clone());
            if let Some(body) = request.get("body") {
                subset.insert("body".into(), body.clone());
            }
            Value::Object(subset)
        })
        .collect();
    assert_canonical(
        "fifth requests",
        json!(fifth_requests),
        &Value::Array(expected_fifth),
    );

    // (6) Missing client info during code exchange.
    let fresh = stub_provider();
    let error = authorize_mcp(
        fresh.as_ref(),
        OAuthFlowOptions {
            server_url: "https://rs.example/mcp".to_string(),
            authorization_code: Some("c".to_string()),
            fetch: Some(Arc::clone(&flow_fetch)),
            ..OAuthFlowOptions::default()
        },
    )
    .await
    .expect_err("missing client");
    assert_eq!(
        error.to_string(),
        expected["missingClientDuringExchange"].as_str().unwrap()
    );
}

// ---------------------------------------------------------------------------
// oauth_provider
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn oauth_provider_matches_the_capture() {
    let _guard = scenario_lock().await;
    crate::mcp::test_rng::reset();
    let expected = &oracle()["oauth_provider"];
    let store = Arc::new(MemoryOAuthStateStore::new());
    let redirects = Arc::new(Mutex::new(Vec::new()));
    let redirect_log = Arc::clone(&redirects);
    let provider = McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: "https://rs.example/mcp/".to_string(),
        redirect_url: "http://localhost:9911/callback".to_string(),
        client_metadata: serde_json::from_value(json!({ "client_name": "pi" })).unwrap(),
        store: Some(store.clone()),
        on_redirect: Arc::new(move |url: Url| {
            let log = Arc::clone(&redirect_log);
            Box::pin(async move {
                log.lock().expect("redirects").push(url.to_string());
            }) as BoxFuture<'static, ()>
        }),
        now_ms: Some(Arc::new(|| FIXED_NOW)),
        client_id: None,
        client_secret: None,
        client_metadata_document: None,
    });
    assert_canonical(
        "client metadata defaults",
        Value::Object(provider.client_metadata().raw.clone()),
        &expected["clientMetadataDefaults"],
    );
    assert_eq!(
        provider.redirect_url(),
        expected["redirectUrl"].as_str().unwrap()
    );
    // state() generates a deterministic hex string from the stubbed RNG.
    let state = provider.state().await.expect("state");
    assert_eq!(state, expected["state"].as_str().unwrap());
    assert_eq!(
        provider.state().await.expect("state again"),
        expected["stateReused"].as_str().unwrap()
    );
    // code verifier round trip; missing verifier errors.
    let error = provider.code_verifier().await.expect_err("no verifier");
    assert_eq!(error.to_string(), expected["noVerifier"].as_str().unwrap());
    provider
        .save_code_verifier("v-123".to_string())
        .await
        .expect("save verifier");
    assert_eq!(
        provider.code_verifier().await.expect("verifier"),
        expected["verifier"].as_str().unwrap()
    );
    // tokens with expiry pinned against FIXED_NOW.
    provider
        .save_tokens(OAuthTokens {
            access_token: "a".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: Some(60.0),
            scope: None,
            refresh_token: None,
            id_token: None,
        })
        .await
        .expect("save tokens");
    let tokens = provider.tokens().await.expect("tokens");
    assert_canonical("tokens", tokens.to_value(), &expected["tokens"]);
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "raw state",
        Value::Object(raw.raw().clone()),
        &expected["rawState"],
    );
    // tokens without expiry clears tokensExpireAt.
    provider
        .save_tokens(OAuthTokens {
            access_token: "b".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: None,
            id_token: None,
        })
        .await
        .expect("save tokens");
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "raw state no expiry",
        Value::Object(raw.raw().clone()),
        &expected["rawStateNoExpiry"],
    );
    // discovery state + client information.
    provider
        .save_client_information(client_information_from(json!({
            "client_id": "c1", "client_secret": "s1",
        })))
        .await
        .expect("save client");
    let client = provider.client_information().await.expect("client");
    assert_canonical(
        "client information",
        Value::Object(client.raw().clone()),
        &expected["clientInformation"],
    );
    provider
        .save_discovery_state(OAuthDiscoveryState {
            authorization_server_url: "https://as.example".to_string(),
            authorization_server_metadata: None,
            resource_metadata: None,
            resource_metadata_url: None,
        })
        .await
        .expect("save discovery");
    let discovery = provider.discovery_state().await.expect("discovery");
    assert_canonical("discovery", discovery.to_value(), &expected["discovery"]);
    // invalidateCredentials kinds.
    provider
        .invalidate_credentials(CredentialKind::Tokens)
        .await
        .expect("invalidate tokens");
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "after tokens invalidation",
        Value::Object(raw.raw().clone()),
        &expected["afterTokensInvalidation"],
    );
    provider
        .invalidate_credentials(CredentialKind::Verifier)
        .await
        .expect("invalidate verifier");
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "after verifier invalidation",
        Value::Object(raw.raw().clone()),
        &expected["afterVerifierInvalidation"],
    );
    provider
        .invalidate_credentials(CredentialKind::Client)
        .await
        .expect("invalidate client");
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "after client invalidation",
        Value::Object(raw.raw().clone()),
        &expected["afterClientInvalidation"],
    );
    provider
        .invalidate_credentials(CredentialKind::Discovery)
        .await
        .expect("invalidate discovery");
    let raw = store.load().await.expect("raw state");
    assert_canonical(
        "after discovery invalidation",
        Value::Object(raw.raw().clone()),
        &expected["afterDiscoveryInvalidation"],
    );
    // Server-URL isolation: state saved under another URL is ignored.
    let other_store = Arc::new(MemoryOAuthStateStore::new());
    other_store
        .save(McpOAuthState::from_raw(
            serde_json::from_value(json!({
                "serverUrl": "https://other.example",
                "tokens": { "access_token": "leak", "token_type": "Bearer" },
                "codeVerifier": "leak-v",
            }))
            .unwrap(),
        ))
        .await;
    let isolated = McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: "https://rs.example/mcp".to_string(),
        redirect_url: "http://localhost:9911/callback".to_string(),
        client_metadata: serde_json::from_value(json!({ "client_name": "pi" })).unwrap(),
        store: Some(other_store),
        on_redirect: Arc::new(|_url: Url| Box::pin(async {})),
        now_ms: Some(Arc::new(|| FIXED_NOW)),
        client_id: None,
        client_secret: None,
        client_metadata_document: None,
    });
    assert_eq!(isolated.tokens().await, None);
    let error = isolated
        .code_verifier()
        .await
        .expect_err("isolated verifier");
    assert_eq!(
        error.to_string(),
        expected["isolatedVerifier"].as_str().unwrap()
    );
    // Configured client id/secret bypasses registration.
    let configured = McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: "https://rs.example/mcp".to_string(),
        redirect_url: "http://localhost:9911/callback".to_string(),
        client_metadata: serde_json::from_value(json!({ "client_name": "pi" })).unwrap(),
        store: None,
        on_redirect: Arc::new(|_url: Url| Box::pin(async {})),
        now_ms: Some(Arc::new(|| FIXED_NOW)),
        client_id: Some("fixed-id".to_string()),
        client_secret: Some("fixed-secret".to_string()),
        client_metadata_document: None,
    });
    let configured_client = configured
        .client_information()
        .await
        .expect("configured client");
    assert_canonical(
        "configured client",
        Value::Object(configured_client.raw().clone()),
        &expected["configuredClient"],
    );
    assert_canonical(
        "configured metadata",
        Value::Object(configured.client_metadata().raw.clone()),
        &expected["configuredMetadata"],
    );
    // Explicit client metadata keys are kept in place.
    let explicit = McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: "https://rs.example/mcp".to_string(),
        redirect_url: "http://localhost:9911/callback".to_string(),
        client_metadata: serde_json::from_value(json!({
            "client_name": "pi",
            "grant_types": ["authorization_code"],
            "token_endpoint_auth_method": "client_secret_basic",
            "redirect_uris": ["http://cb/1"],
        }))
        .unwrap(),
        store: None,
        on_redirect: Arc::new(|_url: Url| Box::pin(async {})),
        now_ms: Some(Arc::new(|| FIXED_NOW)),
        client_id: None,
        client_secret: None,
        client_metadata_document: None,
    });
    assert_canonical(
        "explicit metadata",
        Value::Object(explicit.client_metadata().raw.clone()),
        &expected["explicitMetadata"],
    );
    assert!(redirects.lock().expect("redirects").is_empty());
}

// ---------------------------------------------------------------------------
// oauth_adapt_provider
// ---------------------------------------------------------------------------

/// A minimal never-delivering response body for challenge responses.
fn empty_stream() -> ByteStream {
    Box::pin(futures::stream::empty())
}

fn challenge_response(status: u16, www_authenticate: &str) -> FetchResponse {
    FetchResponse {
        status,
        headers: vec![("www-authenticate".to_string(), www_authenticate.to_string())],
        body: empty_stream(),
    }
}

#[tokio::test]
async fn oauth_adapt_provider_matches_the_capture() {
    let _guard = scenario_lock().await;
    crate::mcp::test_rng::reset();
    let expected = &oracle()["oauth_adapt_provider"];
    let provider = stub_provider();
    {
        let mut state = provider.inner.lock().expect("stub");
        state.token_set = Some(OAuthTokens {
            access_token: "tok-1".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: None,
            id_token: None,
        });
        state.client = Some(client_information_from(json!({ "client_id": "c-1" })));
        state.discovery = Some(OAuthDiscoveryState {
            authorization_server_url: "https://as.example".to_string(),
            authorization_server_metadata: None,
            resource_metadata: None,
            resource_metadata_url: None,
        });
    }
    let adapted = adapt_oauth_provider(provider.clone());
    assert_eq!(
        adapted.token().await,
        Some(expected["token"].as_str().unwrap().to_string())
    );

    // The AS metadata the challenge fetch answers with.
    let metadata_json = as_metadata();
    let metadata_fetch: McpFetch = Arc::new(move |_request: FetchRequest| {
        let body = metadata_json.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Box::pin(futures::stream::iter(std::iter::once(Ok(
                    stringify(&body).into_bytes()
                )))),
            })
        })
    });

    // REDIRECT result -> McpOAuthAuthorizationRequiredError.
    let authorize_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let calls = Arc::clone(&authorize_calls);
        let metadata_fetch_for_wrap = Arc::clone(&metadata_fetch);
        let fetch: McpFetch = Arc::new(move |request: FetchRequest| {
            let inner = Arc::clone(&metadata_fetch_for_wrap);
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            inner(request)
        });
        let error = adapted
            .on_unauthorized(UnauthorizedContext {
                response: challenge_response(401, "Bearer"),
                server_url: Url::parse("https://rs.example/mcp").unwrap(),
                fetch,
                token: Some("tok-1".to_string()),
            })
            .await
            .expect_err("authorization required");
        assert_eq!(
            error.to_string(),
            expected["authorizationRequired"]["message"]
                .as_str()
                .unwrap()
        );
        let McpClientError::OAuth(OAuthFlowError::AuthorizationRequired(_)) = &error else {
            panic!("expected McpOAuthAuthorizationRequiredError, got {error:?}");
        };
    }
    assert_eq!(
        authorize_calls.load(std::sync::atomic::Ordering::SeqCst),
        expected["authorizeCalls"].as_u64().unwrap() as usize
    );

    // Stale token: another request already refreshed -> no authorize call.
    {
        let mut state = provider.inner.lock().expect("stub");
        state.token_set = Some(OAuthTokens {
            access_token: "tok-2".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: None,
            id_token: None,
        });
    }
    let authorize_calls_2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let calls = Arc::clone(&authorize_calls_2);
        let metadata_fetch_for_wrap = Arc::clone(&metadata_fetch);
        let fetch: McpFetch = Arc::new(move |request: FetchRequest| {
            let inner = Arc::clone(&metadata_fetch_for_wrap);
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            inner(request)
        });
        adapted
            .on_unauthorized(UnauthorizedContext {
                response: challenge_response(401, "Bearer"),
                server_url: Url::parse("https://rs.example/mcp").unwrap(),
                fetch,
                token: Some("tok-1".to_string()),
            })
            .await
            .expect("stale token: no refresh");
    }
    assert_eq!(
        authorize_calls_2.load(std::sync::atomic::Ordering::SeqCst),
        expected["staleTokenAuthorizeCalls"].as_u64().unwrap() as usize
    );

    // insufficient_scope skips refresh and goes straight to the redirect.
    {
        let mut state = provider.inner.lock().expect("stub");
        state.token_set = Some(OAuthTokens {
            access_token: "tok-3".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: None,
            scope: None,
            refresh_token: Some("r-3".to_string()),
            id_token: None,
        });
    }
    let error = adapted
        .on_unauthorized(UnauthorizedContext {
            response: challenge_response(
                403,
                "Bearer error=\"insufficient_scope\", scope=\"more\"",
            ),
            server_url: Url::parse("https://rs.example/mcp").unwrap(),
            fetch: Arc::clone(&metadata_fetch),
            token: Some("tok-3".to_string()),
        })
        .await
        .expect_err("insufficient scope");
    assert_eq!(
        error.to_string(),
        expected["insufficientScope"]["message"].as_str().unwrap()
    );
    assert_eq!(
        provider
            .inner
            .lock()
            .expect("stub")
            .authorization_url
            .as_ref()
            .expect("redirect")
            .to_string(),
        expected["insufficientScopeRedirect"].as_str().unwrap()
    );

    // With no stored token at all the challenge path still authorizes once —
    // and the fresh stub's registration answer (the AS metadata, without a
    // client_id) fails parsing like the capture's plain Error.
    let fresh = stub_provider();
    fresh.inner.lock().expect("stub").discovery = Some(OAuthDiscoveryState {
        authorization_server_url: "https://as.example".to_string(),
        authorization_server_metadata: None,
        resource_metadata: None,
        resource_metadata_url: None,
    });
    let adapted_fresh = adapt_oauth_provider(fresh.clone());
    let fresh_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let calls = Arc::clone(&fresh_calls);
        let metadata_fetch_for_wrap = Arc::clone(&metadata_fetch);
        let fetch: McpFetch = Arc::new(move |request: FetchRequest| {
            let inner = Arc::clone(&metadata_fetch_for_wrap);
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            inner(request)
        });
        let error = adapted_fresh
            .on_unauthorized(UnauthorizedContext {
                response: challenge_response(401, ""),
                server_url: Url::parse("https://rs.example/mcp").unwrap(),
                fetch,
                token: None,
            })
            .await
            .expect_err("fresh authorization required");
        // The capture pins the error class ("Error" — a plain failure, not
        // the authorization-required marker).
        let McpClientError::OAuth(flow_error) = &error else {
            panic!("expected an OAuth flow error, got {error:?}");
        };
        assert!(
            !matches!(flow_error, OAuthFlowError::AuthorizationRequired(_)),
            "the fresh path fails with a plain error"
        );
    }
    assert_eq!(
        fresh_calls.load(std::sync::atomic::Ordering::SeqCst),
        expected["freshAuthorizeCalls"].as_u64().unwrap() as usize
    );
}

// ---------------------------------------------------------------------------
// oauth_callback_server (real loopback listener, raw sockets)
// ---------------------------------------------------------------------------

/// One HTTP GET over a raw socket; returns (status, headers, body).
async fn raw_get(port: u16, path_and_query: &str) -> (u16, Vec<(String, String)>, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to callback server");
    let request =
        format!("GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request written");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("response read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("response head");
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("status code");
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').expect("header");
            (name.trim().to_ascii_lowercase(), value.trim().to_string())
        })
        .collect();
    (status, headers, body.to_string())
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value.as_str())
}

/// `http://host:PORT<suffix>` -> the bound port.
fn port_of(redirect_url: &str, suffix: &str) -> u16 {
    let clean_suffix = suffix.replace(":PORT", "");
    let prefix = redirect_url
        .strip_suffix(&clean_suffix)
        .unwrap_or_else(|| panic!("redirect url {redirect_url} ends with {clean_suffix}"));
    let host_port = prefix.trim_start_matches("http://").trim_end_matches(':');
    host_port
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .expect("redirect port")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_callback_server_matches_the_capture() {
    let expected = &oracle()["oauth_callback_server"];
    let server = OAuthCallbackServer::listen(OAuthCallbackServerOptions {
        port: Some(0),
        ..OAuthCallbackServerOptions::default()
    })
    .await
    .expect("listen");
    let redirect_url = server.redirect_url().to_string();
    assert!(
        redirect_url.ends_with("/callback") && redirect_url.starts_with("http://127.0.0.1:"),
        "redirect url matches the pinned shape: {redirect_url}"
    );
    let port = port_of(&redirect_url, "/callback");

    // Happy path: the page answers while the waiter resolves.
    let pending = server.wait_for_callback("state-1", None);
    let page = tokio::spawn(raw_get(
        port,
        "/callback?code=xyz&state=state-1&iss=https://as.example",
    ));
    let callback = pending.await.expect("callback");
    let (status, headers, body) = page.await.expect("page");
    assert_canonical(
        "callback",
        json!({ "code": callback.code, "state": callback.state, "iss": callback.iss }),
        &expected["callback"],
    );
    assert_eq!(
        status,
        expected["okPage"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(
        header_value(&headers, "content-type"),
        Some(
            expected["okPage"]["headers"]["content-type"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(body, expected["okPage"]["body"].as_str().unwrap());

    // Error callback.
    let pending2 = server.wait_for_callback("state-2", None);
    let page2 = tokio::spawn(raw_get(
        port,
        "/callback?state=state-2&error=access_denied&error_description=User%20said%20no",
    ));
    let error = match pending2.await {
        Err(error) => error,
        Ok(callback) => panic!("unexpected callback {callback:?}"),
    };
    assert_eq!(error, expected["errorCallback"].as_str().unwrap());
    let (status, headers, body) = page2.await.expect("page");
    assert_eq!(
        status,
        expected["errorPage"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(
        header_value(&headers, "content-type"),
        Some(
            expected["errorPage"]["headers"]["content-type"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(body, expected["errorPage"]["body"].as_str().unwrap());

    // Unknown state -> 400 page.
    let (status, _headers, body) = raw_get(port, "/callback?code=1&state=nope").await;
    assert_eq!(
        status,
        expected["unknownState"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(body, expected["unknownState"]["body"].as_str().unwrap());
    // Missing state entirely.
    let (status, _headers, body) = raw_get(port, "/callback?code=1").await;
    assert_eq!(
        status,
        expected["missingState"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(body, expected["missingState"]["body"].as_str().unwrap());
    // Wrong path -> 404.
    let (status, _headers, body) = raw_get(port, "/other?state=state-1").await;
    assert_eq!(
        status,
        expected["wrongPath"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(body, expected["wrongPath"]["body"].as_str().unwrap());
    // Missing code -> 400 and rejection.
    let pending3 = server.wait_for_callback("state-3", None);
    let page3 = tokio::spawn(raw_get(port, "/callback?state=state-3"));
    let error = match pending3.await {
        Err(error) => error,
        Ok(callback) => panic!("unexpected callback {callback:?}"),
    };
    assert_eq!(error, expected["missingCodeCallback"].as_str().unwrap());
    let (status, _headers, body) = page3.await.expect("page");
    assert_eq!(
        status,
        expected["missingCodePage"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(body, expected["missingCodePage"]["body"].as_str().unwrap());
    // Duplicate pending state errors (upstream throws synchronously; the
    // port's check runs when the duplicate future is first polled). The
    // first state-4 waiter stays unresolved until the server closes.
    let _pending4 = server.wait_for_callback("state-4", None);
    let duplicate = server.wait_for_callback("state-4", None).await;
    assert_eq!(
        duplicate.expect_err("duplicate"),
        expected["duplicateState"].as_str().unwrap()
    );

    // renderPage HTML rendering with status passthrough.
    let server2 = OAuthCallbackServer::listen(OAuthCallbackServerOptions {
        port: Some(0),
        path: Some("/cb".to_string()),
        host: Some("127.0.0.1".to_string()),
        redirect_host: Some("localhost".to_string()),
        render_page: Some(Arc::new(|page: &OAuthCallbackPage| match page {
            OAuthCallbackPage::Ok => "<html>OK</html>".to_string(),
            OAuthCallbackPage::Error { message, .. } => format!("<html>NO: {message}</html>"),
        })),
        ..OAuthCallbackServerOptions::default()
    })
    .await
    .expect("listen");
    let custom_redirect = server2.redirect_url().to_string();
    assert!(
        custom_redirect.starts_with("http://localhost:") && custom_redirect.ends_with("/cb"),
        "custom redirect url: {custom_redirect}"
    );
    let port2 = port_of(&custom_redirect, "/cb");
    let pending5 = server2.wait_for_callback("s", None);
    let page5 = tokio::spawn(raw_get(port2, "/cb?code=1&state=s"));
    let callback = pending5.await.expect("custom callback");
    let (status, headers, body) = page5.await.expect("page");
    assert_canonical(
        "custom callback",
        json!({ "code": callback.code, "state": callback.state }),
        &expected["customCallback"],
    );
    assert_eq!(
        status,
        expected["customOkPage"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(
        header_value(&headers, "content-type"),
        Some(
            expected["customOkPage"]["headers"]["content-type"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(
        header_value(&headers, "cache-control"),
        Some(
            expected["customOkPage"]["headers"]["cache-control"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(body, expected["customOkPage"]["body"].as_str().unwrap());
    let pending6 = server2.wait_for_callback("s2", None);
    let page6 = tokio::spawn(raw_get(port2, "/cb?state=s2&error=nope"));
    match pending6.await {
        Err(_) => {}
        Ok(callback) => panic!("unexpected callback {callback:?}"),
    }
    let (_status, _headers, body) = page6.await.expect("page");
    assert_eq!(body, expected["customErrorPage"]["body"].as_str().unwrap());
    // close() rejects pending waiters.
    let pending7 = server2.wait_for_callback("s3", None);
    let (error, close_result) = tokio::join!(pending7, server2.close());
    let error = match error {
        Err(error) => error,
        Ok(callback) => panic!("unexpected callback {callback:?}"),
    };
    assert_eq!(error, expected["closedPending"].as_str().unwrap());
    close_result.expect("close");
    server.close().await.expect("close");
}
