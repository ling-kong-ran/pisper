//! Google ADC (Application Default Credentials) service-account token
//! minting for the Vertex AI adapter — the port of the
//! `google-auth-library` credential materialization behind upstream's ADC
//! client construction (`packages/ai/src/api/google-vertex.ts`
//! `buildGoogleAuthOptions` hands `GOOGLE_APPLICATION_CREDENTIALS` to
//! `@google/genai`, whose `GoogleAuth` (js-genai v2.21.0 `_node_auth.ts`)
//! builds a `JWT` client for service-account keys with the
//! `REQUIRED_VERTEX_AI_SCOPE` = `https://www.googleapis.com/auth/cloud-platform`).
//!
//! The pinned wire behavior (google-auth-library v10.3.0 `jwtclient.ts` →
//! `gtoken` v8.0.0 `index.ts` + `jws` v4.0.0):
//! - Key file: a JSON service-account key requires `client_email` and
//!   `private_key` (upstream `JWT.fromJSON` error messages are mirrored
//!   verbatim); `project_id` and `token_uri` are optional (ADC key files
//!   carry `project_id`; `token_uri` on Google-issued keys is always
//!   `https://oauth2.googleapis.com/token`, which is also gtoken's
//!   hardcoded fallback when the field is absent).
//! - JWT assertion: header `{"alg":"RS256"}` exactly (gtoken passes that
//!   header to `jws.sign`, which never adds `typ`); claims in gtoken's
//!   insertion order `iss`, `scope`, `aud`, `exp`, `iat`, `sub` with
//!   `iss` = `sub` = `client_email`, `scope` = the Vertex scope,
//!   `aud` = `token_uri`, `iat` = now, `exp` = `iat + 3600` — RS256
//!   (RSASSA-PKCS1-v1_5 over SHA-256), base64url without padding.
//! - Token exchange: `POST token_uri` with an
//!   `application/x-www-form-urlencoded` body
//!   `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion=<jwt>`;
//!   the response `{access_token, expires_in, ...}` becomes the bearer
//!   credential (`Authorization: Bearer <token>`) with
//!   `expires_at = (iat + expires_in) * 1000`. A JSON error body renders as
//!   gtoken does: `{error}: {error_description}` (the description part only
//!   when present).
//! - Caching: google-auth-library refreshes when
//!   `expiry_date <= now + DEFAULT_EAGER_REFRESH_THRESHOLD_MILLIS`
//!   (5 minutes, `authclient.ts:156`). Upstream constructs a fresh
//!   `GoogleGenAI` client per `stream()` call and therefore re-reads the key
//!   file and re-mints on every request; the port caches process-wide per
//!   key-file identity (path + mtime + size) until the 5-minute threshold —
//!   an M2f binding (cache until near-expiry), so a swapped key file takes
//!   effect on the next refresh rather than immediately.
//!
//! Deviations from upstream, all disclosed:
//! - Upstream gtoken hardcodes `GOOGLE_TOKEN_URL` for both the `aud` claim
//!   and the POST; the port uses the key file's `token_uri` for both
//!   (falling back to `GOOGLE_TOKEN_URL`). Identical bytes for every real
//!   Google-issued key, and it lets the token endpoint be a test double.
//! - Upstream omits `sub` entirely when `JWT.subject` is unset (pi never
//!   sets domain-wide delegation); the port sets `sub = client_email` per
//!   the M2f binding. RFC 7523/Google accept `sub = iss` for service
//!   accounts.
//! - A token response without `expires_in` is treated as never-cacheable
//!   (minted per request) rather than cached forever (upstream's
//!   `isTokenExpiring` treats a missing expiry as not-expiring).
//! - Error messages for unreadable/unparseable key files and unparseable
//!   private keys are port-authored (upstream surfaces raw `fs`/JSON
//!   parse/node-crypto errors); the `fromJSON` field-validation messages
//!   are mirrored verbatim.
//! - The gcloud-variant ADC (no `GOOGLE_APPLICATION_CREDENTIALS` key file —
//!   `gcloud auth application-default login` state or the metadata server)
//!   has no port implementation: invoking the gcloud CLI is out of scope,
//!   and an `authorized_user` key file (which IS gcloud login state) stays
//!   on the caller's named error. Ruling (M2d/M2f): only the service-account
//!   key file mints.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use rsa::pkcs1v15::{Signature, SigningKey};
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, RsaPrivateKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::ai::auth::oauth::pkce::base64url_encode;
use crate::ai::now_ms;

/// js-genai v2.21.0 `_node_auth.ts` `REQUIRED_VERTEX_AI_SCOPE`: the scope
/// the SDK puts on every Vertex ADC client.
pub(crate) const REQUIRED_VERTEX_AI_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// gtoken `GOOGLE_TOKEN_URL`: the hardcoded `aud` claim and POST target
/// upstream, and the port's fallback when the key file omits `token_uri`.
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// gtoken `#requestToken`: `exp = iat + 3600`.
const MAX_TOKEN_LIFETIME_SECS: u64 = 3600;

/// google-auth-library `DEFAULT_EAGER_REFRESH_THRESHOLD_MILLIS`
/// (`authclient.ts:156`): refresh when the cached token is within five
/// minutes of expiry.
const EAGER_REFRESH_THRESHOLD_MS: i64 = 5 * 60 * 1000;

/// The RFC 7523 grant type (gtoken's literal).
const JWT_BEARER_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// The error message for the gcloud ADC variant, owned by the vertex
/// adapter's named-error surface; an `authorized_user` key file (gcloud
/// login state) resolves to the same named error as a missing key file.
pub(crate) const GCLOUD_ADC_NAMED_ERROR: &str = "Vertex AI ADC authentication (gcloud application-default login) is not supported by this port; set GOOGLE_CLOUD_API_KEY or a provider API key instead";

/// The service-account subset of a `GOOGLE_APPLICATION_CREDENTIALS` key file
/// the minting needs (upstream `JWT.fromJSON` + gtoken `getCredentials`).
#[derive(Debug, Clone)]
pub(crate) struct ServiceAccountKey {
    /// `client_email`: the JWT `iss`/`sub`.
    pub client_email: String,
    /// `private_key`: the PEM (PKCS#8 on Google-issued keys; PKCS#1 is
    /// accepted like node crypto).
    pub private_key: String,
    /// `token_uri`: the `aud` claim and POST target; gtoken's
    /// `GOOGLE_TOKEN_URL` when the file omits it.
    pub token_uri: String,
    /// `project_id`: ADC key files carry the owning project (upstream
    /// `JWT.fromJSON` reads it into `projectId`). Parsed for parity; pi's
    /// vertex project resolution is env/options-only upstream
    /// (`resolveProject` throws before credentials are read), so the field
    /// is never consulted by the port's resolution either.
    #[allow(dead_code)]
    pub project_id: Option<String>,
}

/// The parsed `GOOGLE_APPLICATION_CREDENTIALS` credential.
#[derive(Debug, Clone)]
pub(crate) enum AdcCredential {
    /// A service-account key: mints RS256 JWTs.
    ServiceAccount(ServiceAccountKey),
    /// `type: "authorized_user"` — gcloud `application-default login` state;
    /// no port implementation (named error upstream of here).
    GcloudLogin,
}

/// A minted access token with its upstream expiry bookkeeping
/// (gtoken `expiresAt`).
#[derive(Debug, Clone)]
struct AdcToken {
    access_token: String,
    /// `(iat + expires_in) * 1000`; `None` when the response carried no
    /// `expires_in` (never cached — see the module docs).
    expires_at_ms: Option<i64>,
}

impl AdcToken {
    /// google-auth-library `isTokenExpiring` with the default 5-minute
    /// eager-refresh threshold.
    fn is_expiring(&self, now_ms: i64) -> bool {
        match self.expires_at_ms {
            Some(expires_at_ms) => expires_at_ms <= now_ms + EAGER_REFRESH_THRESHOLD_MS,
            // Port deviation: no expiry means no caching (upstream would
            // keep the token forever).
            None => true,
        }
    }
}

/// The key-file JSON shape (`#[serde(default)]` everywhere: upstream
/// validates fields one at a time so each missing field gets its own
/// message).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ServiceAccountKeyFile {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    client_email: Option<String>,
    #[serde(default)]
    private_key: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

/// Read and classify the `GOOGLE_APPLICATION_CREDENTIALS` key file.
/// Field validation mirrors upstream `JWT.fromJSON` verbatim; unreadable
/// files and non-JSON content get port-authored messages (see the module
/// docs).
pub(crate) fn read_adc_credential(path: &str) -> Result<AdcCredential, String> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        format!("Could not read the GOOGLE_APPLICATION_CREDENTIALS key file \"{path}\": {error}")
    })?;
    let file: ServiceAccountKeyFile = serde_json::from_str(&contents).map_err(|error| {
        format!("Could not parse the GOOGLE_APPLICATION_CREDENTIALS key file \"{path}\" as JSON: {error}")
    })?;
    if file.r#type.as_deref() == Some("authorized_user") {
        return Ok(AdcCredential::GcloudLogin);
    }
    let Some(client_email) = file.client_email else {
        return Err("The incoming JSON object does not contain a client_email field".to_string());
    };
    let Some(private_key) = file.private_key else {
        return Err("The incoming JSON object does not contain a private_key field".to_string());
    };
    Ok(AdcCredential::ServiceAccount(ServiceAccountKey {
        client_email,
        private_key,
        token_uri: file
            .token_uri
            .unwrap_or_else(|| GOOGLE_TOKEN_URL.to_string()),
        project_id: file.project_id,
    }))
}

/// `jws.sign` header: gtoken passes `{alg: 'RS256'}` and jws never adds
/// `typ` — the serialized header is exactly `{"alg":"RS256"}`.
#[derive(Serialize)]
struct JwtHeader<'a> {
    alg: &'a str,
}

/// gtoken claim set in insertion order (`iss, scope, aud, exp, iat, sub`);
/// a serde struct pins the byte order (serde_json maps sort by key).
#[derive(Serialize)]
struct JwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    exp: u64,
    iat: u64,
    sub: &'a str,
}

/// Parse the PEM private key (PKCS#8 `BEGIN PRIVATE KEY` on Google-issued
/// keys, PKCS#1 `BEGIN RSA PRIVATE KEY` accepted like node crypto) into an
/// RS256 `SigningKey`.
fn rs256_signing_key(private_key_pem: &str) -> Result<SigningKey<Sha256>, String> {
    let parsed = if private_key_pem.contains("BEGIN RSA PRIVATE KEY") {
        RsaPrivateKey::from_pkcs1_pem(private_key_pem).map_err(|error| error.to_string())
    } else {
        RsaPrivateKey::from_pkcs8_pem(private_key_pem).map_err(|error| error.to_string())
    };
    let key = parsed
        .map_err(|error| format!("Could not parse the service-account private key: {error}"))?;
    Ok(SigningKey::<Sha256>::new(key))
}

/// Mint the RS256 JWT assertion (gtoken `#requestToken`, jws signing):
/// `base64url(header).base64url(claims).base64url(RS256-signature)` over
/// the first two segments. `iat` is injected so tests pin the claims.
fn mint_jwt_assertion(key: &ServiceAccountKey, iat: u64) -> Result<String, String> {
    let signing_key = rs256_signing_key(&key.private_key)?;
    let header = base64url_encode(
        &serde_json::to_vec(&JwtHeader { alg: "RS256" })
            .map_err(|error| format!("Could not serialize the JWT header: {error}"))?,
    );
    let claims = base64url_encode(
        &serde_json::to_vec(&JwtClaims {
            iss: &key.client_email,
            scope: REQUIRED_VERTEX_AI_SCOPE,
            aud: &key.token_uri,
            exp: iat + MAX_TOKEN_LIFETIME_SECS,
            iat,
            sub: &key.client_email,
        })
        .map_err(|error| format!("Could not serialize the JWT claims: {error}"))?,
    );
    let signing_input = format!("{header}.{claims}");
    // RSASSA-PKCS1-v1_5 over SHA-256 is deterministic; the rng is an API
    // artifact of the signer trait.
    let signature: Signature =
        signing_key.sign_with_rng(&mut rand::rng(), signing_input.as_bytes());
    let signature = base64url_encode(&signature.to_vec());
    Ok(format!("{signing_input}.{signature}"))
}

/// Percent-encode one urlencoded key/value (the WHATWG
/// `application/x-www-form-urlencoded` serializer: everything but ASCII
/// alphanumerics and `*-._` is percent-encoded, space becomes `+`).
fn urlencoded_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The `URLSearchParams` body gtoken sends:
/// `grant_type=<grant>&assertion=<jwt>`.
fn urlencoded_body(assertion: &str) -> String {
    format!(
        "grant_type={}&assertion={}",
        urlencoded_component(JWT_BEARER_GRANT_TYPE),
        urlencoded_component(assertion)
    )
}

/// The token-endpoint response subset gtoken reads.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange the assertion for an access token (gtoken `#requestToken`'s
/// POST) and stamp the expiry the way gtoken does:
/// `expiresAt = (iat + expires_in) * 1000`.
async fn exchange_token(token_uri: &str, assertion: &str, iat: u64) -> Result<AdcToken, String> {
    let response = crate::ai::api::http_client()
        .post(token_uri)
        .header(
            "content-type",
            "application/x-www-form-urlencoded;charset=UTF-8",
        )
        .body(urlencoded_body(assertion))
        .send()
        .await
        .map_err(|error| format!("Could not reach the token endpoint \"{token_uri}\": {error}"))?;
    let status = response.status();
    let body_text = response
        .text()
        .await
        .map_err(|error| format!("Could not read the token response: {error}"))?;
    let body: TokenResponse = serde_json::from_str(&body_text).map_err(|error| {
        format!("Could not parse the token response (status {status}) as JSON: {error}")
    })?;
    if !status.is_success() || body.error.is_some() {
        // gtoken's error rendering: `{error}: {error_description}` (the
        // description part only when present).
        let message = match (&body.error, &body.error_description) {
            (Some(error), Some(description)) => format!("{error}: {description}"),
            (Some(error), None) => error.clone(),
            (None, _) => format!("status {status}"),
        };
        return Err(format!("Token exchange failed: {message}"));
    }
    let Some(access_token) = body.access_token else {
        return Err("Token exchange response did not contain an access_token".to_string());
    };
    let expires_at_ms = body
        .expires_in
        .map(|expires_in| (iat + expires_in) as i64 * 1000);
    Ok(AdcToken {
        access_token,
        expires_at_ms,
    })
}

/// The cache key: the key-file identity (path + mtime + size) so a swapped
/// key file gets a fresh cache entry (see the module docs). `None` when the
/// file's identity cannot be statted (the mint then runs uncached and the
/// read error surfaces from the parse).
fn cache_identity(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!(
        "{}\u{1f}{}\u{1f}{}",
        path,
        modified.as_nanos(),
        metadata.len()
    ))
}

/// Process-wide minted-token cache (upstream keeps the equivalent on the
/// per-request `GoogleAuth`/`gtoken` instances; the port is process-wide per
/// the M2f cache binding). The mutex is held across the mint so concurrent
/// requests share one token exchange (upstream's `#inFlightRequest`).
static ADC_TOKEN_CACHE: LazyLock<Mutex<HashMap<String, AdcToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The bearer access token for the `GOOGLE_APPLICATION_CREDENTIALS`
/// service-account key file: cached until the 5-minute eager-refresh
/// threshold, minted (RS256 JWT assertion → token exchange) otherwise.
/// Errors are stream-error messages: the named gcloud error for
/// `authorized_user` files, upstream-verbatim field validation, and
/// port-authored IO/parse messages (see the module docs).
pub(crate) async fn adc_access_token(key_file: &str) -> Result<String, String> {
    let identity = cache_identity(key_file);
    let mut cache = ADC_TOKEN_CACHE.lock().await;
    let now = now_ms();
    if let Some(identity) = identity.as_deref() {
        if let Some(token) = cache.get(identity) {
            if !token.is_expiring(now) {
                return Ok(token.access_token.clone());
            }
        }
    }
    let credential = read_adc_credential(key_file)?;
    let key = match credential {
        AdcCredential::GcloudLogin => return Err(GCLOUD_ADC_NAMED_ERROR.to_string()),
        AdcCredential::ServiceAccount(key) => key,
    };
    let iat = (now_ms() / 1000) as u64;
    let assertion = mint_jwt_assertion(&key, iat)?;
    let token = exchange_token(&key.token_uri, &assertion, iat).await?;
    if let Some(identity) = identity {
        cache.insert(identity, token.clone());
    }
    Ok(token.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1v15::VerifyingKey;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::signature::{Keypair, Verifier};
    use serde_json::json;

    /// 64 hex chars — realistic-looking but inert.
    const TEST_ACCESS_TOKEN: &str = "1//fake-access-token-for-tests-0123456789abcdef";

    /// An in-test RSA keypair: the private key as the PKCS#8 PEM a
    /// Google-issued key file embeds, plus the `VerifyingKey` used to check
    /// minted signatures.
    struct TestKey {
        private_key_pem: String,
        verifying_key: VerifyingKey<Sha256>,
    }

    impl TestKey {
        fn generate() -> TestKey {
            let mut rng = rand::rng();
            let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("test keygen");
            let signing_key = SigningKey::<Sha256>::new(private_key.clone());
            let private_key_pem = private_key
                .to_pkcs8_pem(LineEnding::LF)
                .expect("PEM encode")
                .to_string();
            TestKey {
                private_key_pem,
                verifying_key: signing_key.verifying_key(),
            }
        }

        /// A service-account key file with `token_uri` pointed at the test
        /// token endpoint.
        fn key_file_json(&self, token_uri: &str, project_id: Option<&str>) -> serde_json::Value {
            json!({
                "type": "service_account",
                "project_id": project_id,
                "private_key_id": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                "private_key": self.private_key_pem,
                "client_email": "sa@test-project.iam.gserviceaccount.com",
                "client_id": "100000000000000000000",
                "token_uri": token_uri,
            })
        }
    }

    /// Writes the key JSON to its own temp file (unique path = unique cache
    /// identity, so parallel tests never share cached tokens) with
    /// `token_uri` appended to the base; an empty base removes `token_uri`
    /// entirely (the gtoken-fallback case). Returns the path.
    fn write_key_file(key: &TestKey, token_uri_base: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let file = dir.join("sa.json");
        let token_uri = if token_uri_base.is_empty() {
            String::new()
        } else {
            format!("{token_uri_base}/token")
        };
        let mut json = key.key_file_json(&token_uri, Some("adc-test-project"));
        if token_uri.is_empty() {
            json.as_object_mut().unwrap().remove("token_uri");
        }
        std::fs::write(&file, json.to_string()).expect("write key file");
        // The tempdir must outlive the test: leak it (test process exits
        // anyway).
        std::mem::forget(dir);
        file
    }

    /// Mounts the token endpoint on the mock server, returning nothing (the
    /// default mount answers every POST to `/token`).
    async fn mount_token_endpoint(
        server: &wiremock::MockServer,
        body: serde_json::Value,
        status: u16,
    ) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .insert_header("content-type", "application/json")
                    .set_body_string(body.to_string()),
            )
            .mount(server)
            .await;
    }

    fn token_ok(expires_in: u64) -> serde_json::Value {
        json!({
            "access_token": TEST_ACCESS_TOKEN,
            "expires_in": expires_in,
            "token_type": "Bearer",
        })
    }

    /// The env google_vertex reads `GOOGLE_APPLICATION_CREDENTIALS` from:
    /// the scoped `options.env` (the ambient process env must NOT leak into
    /// the resolution).
    #[tokio::test]
    async fn mint_end_to_end_over_wiremock() {
        let key = TestKey::generate();
        let server = wiremock::MockServer::start().await;
        mount_token_endpoint(&server, token_ok(3600), 200).await;
        let key_file = write_key_file(&key, &server.uri());

        // End to end through the public entry: parse → mint → exchange →
        // cache → bearer string.
        let token = adc_access_token(key_file.to_str().unwrap()).await.unwrap();
        assert_eq!(token, TEST_ACCESS_TOKEN);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "exactly one token exchange");
        let request = &requests[0];
        assert_eq!(request.url.path(), "/token");
        assert_eq!(
            body_of(request, "content-type").unwrap(),
            "application/x-www-form-urlencoded;charset=UTF-8"
        );
        let body = String::from_utf8(request.body.clone()).unwrap();
        let (grant_type, assertion) = parse_urlencoded(&body);
        assert_eq!(
            grant_type,
            Some("urn:ietf:params:oauth:grant-type:jwt-bearer".to_string())
        );
        verify_minted_jwt(
            &key,
            &format!("{}/token", server.uri()),
            &assertion.expect("assertion present"),
        );
    }

    #[tokio::test]
    async fn cached_token_skips_the_exchange_until_near_expiry() {
        let key = TestKey::generate();
        let server = wiremock::MockServer::start().await;
        // expires_in = 3600s: far outside the 5-minute threshold, so the
        // second call must be served from the cache.
        mount_token_endpoint(&server, token_ok(3600), 200).await;
        let key_file = write_key_file(&key, &server.uri());
        let path = key_file.to_str().unwrap();

        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "the second call must hit the cache, not the wire"
        );
    }

    #[tokio::test]
    async fn near_expiry_tokens_refresh() {
        let key = TestKey::generate();
        let server = wiremock::MockServer::start().await;
        // expires_in = 1s: inside the 5-minute eager-refresh threshold, so
        // the second call must re-mint.
        mount_token_endpoint(&server, token_ok(1), 200).await;
        let key_file = write_key_file(&key, &server.uri());
        let path = key_file.to_str().unwrap();

        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "a near-expiry token must refresh on the second call"
        );
    }

    #[tokio::test]
    async fn token_responses_without_expires_in_are_never_cached() {
        let key = TestKey::generate();
        let server = wiremock::MockServer::start().await;
        // No `expires_in`: the port treats the token as never-cacheable
        // (module-docs deviation from upstream's isTokenExpiring), so every
        // call re-mints.
        mount_token_endpoint(
            &server,
            json!({"access_token": TEST_ACCESS_TOKEN, "token_type": "Bearer"}),
            200,
        )
        .await;
        let key_file = write_key_file(&key, &server.uri());
        let path = key_file.to_str().unwrap();

        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(adc_access_token(path).await.unwrap(), TEST_ACCESS_TOKEN);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "a token without expires_in must not be cached"
        );
    }

    #[tokio::test]
    async fn key_file_with_no_token_uri_falls_back_to_the_google_token_url() {
        let key = TestKey::generate();
        let key_file = write_key_file(&key, "");
        match read_adc_credential(key_file.to_str().unwrap()).unwrap() {
            AdcCredential::ServiceAccount(key) => {
                assert_eq!(key.token_uri, GOOGLE_TOKEN_URL);
                assert_eq!(key.project_id.as_deref(), Some("adc-test-project"));
                assert_eq!(key.client_email, "sa@test-project.iam.gserviceaccount.com");
            }
            other => panic!("expected a service account, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn authorized_user_files_resolve_to_the_named_gcloud_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("adc.json");
        std::fs::write(
            &file,
            json!({
                "type": "authorized_user",
                "client_id": "id.apps.googleusercontent.com",
                "client_secret": "secret",
                "refresh_token": "token",
                "quota_project_id": "p",
            })
            .to_string(),
        )
        .unwrap();
        // The file IS gcloud login state: the minting entry must refuse with
        // the vertex adapter's named gcloud error, verbatim.
        assert_eq!(
            adc_access_token(file.to_str().unwrap()).await.unwrap_err(),
            GCLOUD_ADC_NAMED_ERROR
        );
    }

    #[tokio::test]
    async fn key_files_missing_required_fields_mirror_upstream_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, json!({"type": "service_account"}).to_string()).unwrap();
        assert_eq!(
            read_adc_credential(path.to_str().unwrap()).unwrap_err(),
            "The incoming JSON object does not contain a client_email field"
        );

        std::fs::write(
            &path,
            json!({"type": "service_account", "client_email": "sa@p.iam.gserviceaccount.com"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            read_adc_credential(path.to_str().unwrap()).unwrap_err(),
            "The incoming JSON object does not contain a private_key field"
        );
    }

    #[tokio::test]
    async fn unreadable_key_files_name_the_path_and_cause() {
        assert!(read_adc_credential("/definitely/not/a/real/key.json")
            .unwrap_err()
            .starts_with(
                "Could not read the GOOGLE_APPLICATION_CREDENTIALS key file \
                     \"/definitely/not/a/real/key.json\":"
            ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-json.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(read_adc_credential(path.to_str().unwrap())
            .unwrap_err()
            .starts_with("Could not parse the GOOGLE_APPLICATION_CREDENTIALS key file"));
    }

    #[test]
    fn jwt_structure_matches_gtoken_byte_for_byte() {
        let key = TestKey::generate();
        let service_account = ServiceAccountKey {
            client_email: "sa@test-project.iam.gserviceaccount.com".to_string(),
            private_key: key.private_key_pem.clone(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            project_id: Some("test-project".to_string()),
        };
        let assertion = mint_jwt_assertion(&service_account, 1_700_000_000).unwrap();
        let segments: Vec<&str> = assertion.split('.').collect();
        assert_eq!(segments.len(), 3, "{assertion}");

        // Header: jws gets `{"alg":"RS256"}` and adds nothing.
        let header = decode_segment(segments[0]);
        assert_eq!(header, json!({"alg": "RS256"}).to_string());

        // Claims: gtoken's insertion order iss, scope, aud, exp, iat, sub.
        let claims = decode_segment(segments[1]);
        assert_eq!(
            claims,
            format!(
                r#"{{"iss":"sa@test-project.iam.gserviceaccount.com","scope":"{scope}","aud":"https://oauth2.googleapis.com/token","exp":1700003600,"iat":1700000000,"sub":"sa@test-project.iam.gserviceaccount.com"}}"#,
                scope = REQUIRED_VERTEX_AI_SCOPE,
            )
        );
    }

    #[test]
    fn minted_signatures_verify_against_the_key_pair() {
        let key = TestKey::generate();
        let service_account = ServiceAccountKey {
            client_email: "sa@test-project.iam.gserviceaccount.com".to_string(),
            private_key: key.private_key_pem.clone(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            project_id: None,
        };
        let assertion = mint_jwt_assertion(&service_account, 1_700_000_000).unwrap();
        let segments: Vec<&str> = assertion.split('.').collect();
        let signing_input = format!("{}.{}", segments[0], segments[1]);
        let signature = Signature::try_from(decode_segment_bytes(segments[2]).as_slice())
            .expect("signature decodes");
        key.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .expect("the minted RS256 signature must verify with the public key");
    }

    #[test]
    fn pkcs1_private_keys_mint_verifiable_jwt_too() {
        // Google-issued keys are PKCS#8; node crypto (jws) also accepts
        // PKCS#1 `BEGIN RSA PRIVATE KEY`, so the parser must too.
        let key = TestKey::generate();
        // Re-encode the same key material as PKCS#1 via the pkcs8 parse.
        let private_key = RsaPrivateKey::from_pkcs8_pem(&key.private_key_pem).expect("parse pkcs8");
        let pkcs1_pem = {
            use rsa::pkcs1::EncodeRsaPrivateKey;
            private_key
                .to_pkcs1_pem(LineEnding::LF)
                .unwrap()
                .to_string()
        };
        assert!(pkcs1_pem.contains("BEGIN RSA PRIVATE KEY"));
        let service_account = ServiceAccountKey {
            client_email: "sa@test-project.iam.gserviceaccount.com".to_string(),
            private_key: pkcs1_pem,
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            project_id: None,
        };
        let assertion = mint_jwt_assertion(&service_account, 1_700_000_000).unwrap();
        let segments: Vec<&str> = assertion.split('.').collect();
        let signing_input = format!("{}.{}", segments[0], segments[1]);
        let signature = Signature::try_from(decode_segment_bytes(segments[2]).as_slice())
            .expect("signature decodes");
        key.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .expect("the PKCS#1-minted signature must verify");
    }

    #[test]
    fn urlencoded_body_matches_the_url_search_params_serializer() {
        assert_eq!(
            urlencoded_body("abcXYZ123-_d.e_f*g"),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&\
             assertion=abcXYZ123-_d.e_f*g"
        );
        // URLSearchParams: space -> '+', '~' is NOT in the URLSearchParams
        // unreserved set (unlike RFC 3986 unreserved), so it is
        // percent-encoded.
        assert_eq!(urlencoded_component("a b~c"), "a+b%7Ec");
    }

    #[tokio::test]
    async fn token_exchange_failures_render_like_gtoken() {
        let key = TestKey::generate();
        let server = wiremock::MockServer::start().await;
        mount_token_endpoint(
            &server,
            json!({
                "error": "invalid_grant",
                "error_description": "Invalid JWT Signature.",
            }),
            400,
        )
        .await;
        let key_file = write_key_file(&key, &server.uri());
        let error = adc_access_token(key_file.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "Token exchange failed: invalid_grant: Invalid JWT Signature."
        );
    }

    // ---- helpers ----

    fn body_of(request: &wiremock::Request, name: &str) -> Option<String> {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// Parses the urlencoded body into (grant_type, assertion).
    fn parse_urlencoded(body: &str) -> (Option<String>, Option<String>) {
        let mut grant_type = None;
        let mut assertion = None;
        for pair in body.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = urlencoded_component_unescape(value);
            match key {
                "grant_type" => grant_type = Some(value),
                "assertion" => assertion = Some(value),
                _ => {}
            }
        }
        (grant_type, assertion)
    }

    fn urlencoded_component_unescape(value: &str) -> String {
        let plus_back = value.replace('+', " ");
        let mut out = Vec::with_capacity(plus_back.len());
        let bytes = plus_back.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                out.push(u8::from_str_radix(&plus_back[index + 1..index + 3], 16).unwrap_or(b'%'));
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn decode_segment(segment: &str) -> String {
        String::from_utf8(decode_segment_bytes(segment)).unwrap()
    }

    /// base64url (no padding) decode — the JWT segment alphabet.
    fn decode_segment_bytes(segment: &str) -> Vec<u8> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = Vec::with_capacity(segment.len() * 3 / 4);
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for byte in segment.bytes() {
            let value = ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .unwrap_or_else(|| panic!("non-base64url byte {byte}"))
                as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }
        out
    }

    /// Full oracle of the minted JWT against the test key: three segments,
    /// gtoken claims, and an RS256 signature that verifies.
    fn verify_minted_jwt(key: &TestKey, token_uri: &str, assertion: &str) {
        let segments: Vec<&str> = assertion.split('.').collect();
        assert_eq!(segments.len(), 3, "{assertion}");
        assert_eq!(
            decode_segment(segments[0]),
            json!({"alg": "RS256"}).to_string()
        );
        let claims: serde_json::Value = serde_json::from_str(&decode_segment(segments[1])).unwrap();
        assert_eq!(claims["iss"], "sa@test-project.iam.gserviceaccount.com");
        assert_eq!(claims["sub"], claims["iss"]);
        assert_eq!(claims["aud"], token_uri);
        assert_eq!(claims["scope"], REQUIRED_VERTEX_AI_SCOPE);
        assert_eq!(claims["exp"], claims["iat"].as_i64().unwrap() + 3600);
        let signing_input = format!("{}.{}", segments[0], segments[1]);
        let signature = Signature::try_from(decode_segment_bytes(segments[2]).as_slice())
            .expect("signature decodes");
        key.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .expect("the minted RS256 signature must verify with the public key");
    }
}
