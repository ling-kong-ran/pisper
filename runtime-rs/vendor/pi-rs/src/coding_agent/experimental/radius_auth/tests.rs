//! Tests for `radius_auth.rs`: pinned to the node oracle captured from the
//! verbatim upstream file
//! (tests/fixtures/experimental_final_oracle/oracle_relay_out.json, `authResolver`
//! section; upstream sources in tests/fixtures/experimental_final_oracle/upstream).

use super::*;

fn resolver(input: Option<AuthInput>) -> RadiusRelayAuthResolver {
    RadiusRelayAuthResolver::new(input, DEFAULT_RADIUS_GATEWAY)
}

fn env_offline() -> ResolveEnvironment<'static> {
    ResolveEnvironment {
        offline: true,
        ..Default::default()
    }
}

#[test]
fn gateway_normalization_matches_the_oracle() {
    // Oracle: defaultGateway, gatewayNormalization.
    assert_eq!(resolver(None).gateway(), "https://radius.pi.dev");
    let normalized: Vec<String> = [
        "radius.example",
        "https://x.dev/",
        "http://y.dev///",
        "https://z.dev",
    ]
    .iter()
    .map(|value| {
        RadiusRelayAuthResolver::new(None, value)
            .gateway()
            .to_string()
    })
    .collect();
    assert_eq!(
        normalized,
        vec![
            "https://radius.example",
            "https://x.dev",
            "http://y.dev",
            "https://z.dev"
        ]
    );
}

#[test]
fn explicit_token_is_trimmed_and_rejected_when_empty() {
    // Oracle: explicitToken {gateway, token: "secret"}; emptyTokenError.
    let explicit = resolver(Some(AuthInput::Token("  secret  ".to_string())));
    let resolved = explicit
        .resolve(
            ResolveOptions { required: false },
            ResolveEnvironment::default(),
            ".",
        )
        .unwrap();
    assert_eq!(
        resolved,
        Some(RadiusRelayAuth {
            gateway: "https://radius.pi.dev".to_string(),
            token: "secret".to_string(),
        })
    );

    let empty = resolver(Some(AuthInput::Token("   ".to_string())));
    let error = empty
        .resolve(
            ResolveOptions { required: true },
            ResolveEnvironment::default(),
            ".",
        )
        .unwrap_err();
    assert_eq!(error, "Radius authentication token must not be empty");
}

#[test]
fn token_files_are_read_trimmed_through_the_path_input() {
    let directory = std::env::temp_dir().join("pi_rust_radius_auth_path");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("token.txt");
    std::fs::write(&path, "file-token\n").unwrap();
    let absolute = path.to_string_lossy().to_string();
    let resolver = resolver(Some(AuthInput::Path(absolute)));
    let resolved = resolver
        .resolve(
            ResolveOptions { required: true },
            ResolveEnvironment::default(),
            ".",
        )
        .unwrap();
    assert_eq!(resolved.unwrap().token, "file-token");
}

#[test]
fn offline_mode_matches_the_oracle_error_and_optional_none() {
    // Oracle: offline.offlineRequired; offlineOptional resolves undefined.
    let required = resolver(Some(AuthInput::Token("secret".to_string())))
        .resolve(ResolveOptions { required: true }, env_offline(), ".")
        .unwrap_err();
    assert_eq!(
        required,
        "Radius relay connections are unavailable in offline mode"
    );
    let optional = resolver(None)
        .resolve(ResolveOptions { required: false }, env_offline(), ".")
        .unwrap();
    assert_eq!(optional, None);
}

#[test]
fn missing_stored_credential_matches_the_oracle_errors() {
    // Oracle: missingCredential.missingRequired / missingOptional (undefined).
    let required = resolver(None)
        .resolve(
            ResolveOptions { required: true },
            ResolveEnvironment::default(),
            ".",
        )
        .unwrap_err();
    assert_eq!(
        required,
        "Radius authentication is required; start Pi and run /login radius, then retry"
    );
    let optional = resolver(None)
        .resolve(
            ResolveOptions { required: false },
            ResolveEnvironment::default(),
            ".",
        )
        .unwrap();
    assert_eq!(optional, None);
}

#[test]
fn stored_credential_is_used_when_present() {
    // Oracle: storedCredential {gateway, token: "stored-token"}.
    let environment = ResolveEnvironment {
        stored_token: Some("stored-token"),
        ..Default::default()
    };
    let resolved = resolver(None)
        .resolve(ResolveOptions { required: false }, environment, ".")
        .unwrap();
    assert_eq!(
        resolved,
        Some(RadiusRelayAuth {
            gateway: "https://radius.pi.dev".to_string(),
            token: "stored-token".to_string(),
        })
    );
}

#[test]
fn explicit_token_wins_over_the_stored_credential() {
    // Upstream returns the explicit token before consulting ModelRuntime.
    let environment = ResolveEnvironment {
        stored_token: Some("stored-token"),
        ..Default::default()
    };
    let resolved = resolver(Some(AuthInput::Token("explicit".to_string())))
        .resolve(ResolveOptions { required: false }, environment, ".")
        .unwrap();
    assert_eq!(resolved.unwrap().token, "explicit");
}

#[test]
fn cancellation_surfaces_the_embedder_abort_reason() {
    // Upstream `options.signal?.throwIfAborted()` checkpoints; the reason
    // text is embedder-owned, so it is passed through verbatim.
    let environment = ResolveEnvironment {
        cancelled: Some("Radius relay connection cancelled".to_string()),
        ..Default::default()
    };
    let error = resolver(None)
        .resolve(ResolveOptions { required: false }, environment, ".")
        .unwrap_err();
    assert_eq!(error, "Radius relay connection cancelled");
}
