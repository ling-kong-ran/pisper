//! Request authentication for the Bedrock ConverseStream endpoint: SigV4 via
//! the `aws-sigv4` crate (the architecture decision's stand-in for the AWS
//! SDK's signing middleware) plus the Bedrock bearer-token scheme
//! (`Authorization: Bearer <token>`, upstream `config.token` +
//! `authSchemePreference: ["httpBearerAuth"]`).
//!
//! Signing surface (what the SDK middleware signs, reproduced here):
//! method + URI + the headers given (host is derived from the URI by the
//! crate), `x-amz-date`, the session token via `x-amz-security-token`, the
//! SHA-256 payload hash in the canonical request, service name `bedrock`,
//! and the resolved region. Headers that must never be caller-controlled
//! (`authorization`, `host`, `x-amz-*`) are filtered before signing — see
//! [`is_reserved_header`].

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use aws_smithy_runtime_api::client::identity::Identity;
use std::time::SystemTime;

/// Upstream service signing name for the Bedrock runtime APIs.
pub const SERVICE_NAME: &str = "bedrock";

/// Static AWS credentials (upstream `config.credentials`): the env-resolved
/// keys, the `AWS_BEDROCK_SKIP_AUTH` dummy pair, or nothing (bearer mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// Upstream `AWS_BEDROCK_SKIP_AUTH=1` dummy credentials (lines 205-211):
/// SigV4 with placeholder keys so local gateways can ignore the signature.
pub fn dummy_credentials() -> AwsCredentials {
    AwsCredentials {
        access_key_id: "dummy-access-key".to_string(),
        secret_access_key: "dummy-secret-key".to_string(),
        session_token: None,
    }
}

/// Header keys that must never be overwritten by caller-supplied headers
/// (upstream `isReservedHeader`, lines 463-468): `host` and `x-amz-*`
/// participate in the SigV4 canonical request, `authorization` is owned by
/// SigV4 or the bearer path. Compared case-insensitively.
pub fn is_reserved_header(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with("x-amz-") || lower == "authorization" || lower == "host"
}

/// Signs one request and returns the headers to attach (authorization,
/// x-amz-date, and x-amz-security-token when a session token is present).
/// `headers` are the headers already on the request (content-type plus any
/// injected custom headers); they join the signed set like upstream's
/// build-step middleware running before SigV4.
pub fn sign_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    region: &str,
    credentials: &AwsCredentials,
    now: SystemTime,
) -> Result<Vec<(String, String)>, String> {
    let credentials = Credentials::new(
        credentials.access_key_id.clone(),
        credentials.secret_access_key.clone(),
        credentials.session_token.clone(),
        None,
        "pi-rust",
    );
    let identity: Identity = credentials.into();
    let v4_params = SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(SERVICE_NAME)
        .time(now)
        .settings(SigningSettings::default())
        .build()
        .map_err(|error| format!("SigV4 signing params error: {error}"))?;
    let signing_params = aws_sigv4::http_request::SigningParams::V4(v4_params);
    let signable = SignableRequest::new(
        method,
        url,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(body),
    )
    .map_err(|error| format!("SigV4 signable request error: {error}"))?;
    let output =
        sign(signable, &signing_params).map_err(|error| format!("SigV4 signing error: {error}"))?;
    let (instructions, _signature) = output.into_parts();
    let (signed_headers, _params) = instructions.into_parts();
    Ok(signed_headers
        .into_iter()
        .map(|header| (header.name().to_string(), header.value().to_string()))
        .collect())
}

/// The bearer-token authorization header value (upstream `config.token`):
/// `Authorization: Bearer <token>` replaces SigV4 entirely.
pub fn bearer_authorization(token: &str) -> (String, String) {
    ("authorization".to_string(), format!("Bearer {token}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        }
    }

    fn test_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_758_240_000)
    }

    #[test]
    fn reserved_header_detection_matches_upstream() {
        for reserved in [
            "authorization",
            "Authorization",
            "host",
            "HOST",
            "x-amz-date",
            "X-Amz-Date",
            "x-amz-security-token",
        ] {
            assert!(is_reserved_header(reserved), "{reserved}");
        }
        for allowed in ["x-custom", "x-bifrost-key", "accept", "content-type"] {
            assert!(!is_reserved_header(allowed), "{allowed}");
        }
    }

    #[test]
    fn signed_headers_carry_authorization_date_and_token() {
        let signed = sign_request(
            "POST",
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/test-model/stream",
            &[("content-type".to_string(), "application/json".to_string())],
            b"{}",
            "us-east-1",
            &AwsCredentials {
                session_token: Some("TOKEN".to_string()),
                ..credentials()
            },
            test_time(),
        )
        .unwrap();
        let names: Vec<&str> = signed.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"authorization"), "{names:?}");
        assert!(names.contains(&"x-amz-date"), "{names:?}");
        assert!(names.contains(&"x-amz-security-token"), "{names:?}");

        let authorization = signed
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .unwrap();
        // The credential scope pins service `bedrock`, the region, and date.
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250919/us-east-1/bedrock/aws4_request, SignedHeaders="),
            "{authorization}"
        );
        let date = signed
            .iter()
            .find(|(name, _)| name == "x-amz-date")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert_eq!(date, "20250919T000000Z");
    }

    #[test]
    fn signing_is_deterministic_per_input() {
        let args = (
            "POST",
            "https://bedrock-runtime.eu-west-1.amazonaws.com/model/m/stream",
            vec![("content-type".to_string(), "application/json".to_string())],
            b"{}".to_vec(),
        );
        let first = sign_request(
            args.0,
            args.1,
            &args.2,
            &args.3,
            "eu-west-1",
            &credentials(),
            test_time(),
        )
        .unwrap();
        let second = sign_request(
            args.0,
            args.1,
            &args.2,
            &args.3,
            "eu-west-1",
            &credentials(),
            test_time(),
        )
        .unwrap();
        assert_eq!(first, second);
        // A different body produces a different signature (payload is signed).
        let other_body = sign_request(
            args.0,
            args.1,
            &args.2,
            b"other",
            "eu-west-1",
            &credentials(),
            test_time(),
        )
        .unwrap();
        assert_ne!(first, other_body);
    }

    #[test]
    fn bearer_authorization_header_shape() {
        let (name, value) = bearer_authorization("bedrock-api-key");
        assert_eq!(name, "authorization");
        assert_eq!(value, "Bearer bedrock-api-key");
    }

    #[test]
    fn dummy_credentials_match_upstream_placeholders() {
        let dummy = dummy_credentials();
        assert_eq!(dummy.access_key_id, "dummy-access-key");
        assert_eq!(dummy.secret_access_key, "dummy-secret-key");
        assert_eq!(dummy.session_token, None);
    }
}
