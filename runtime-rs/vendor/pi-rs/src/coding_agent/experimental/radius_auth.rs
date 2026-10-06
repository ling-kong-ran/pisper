//! Port of upstream `experimental/radius-auth.ts`
//! (sha256 8e3a7e6d8deb37c883c6033bca55c292f407438e032c99046d19f613af4938b4)
//! plus the two value surfaces it consumes from upstream
//! `packages/ai/src/providers/radius-config.ts`
//! (sha256 1f82a7db25753be09374792ae0a3cd63417739018fb69b3212ad8b3b9eb11c21):
//! `DEFAULT_RADIUS_GATEWAY` and `normalizeRadiusGatewayUrl`.
//!
//! Ported: the gateway default + normalization, the resolve decision ladder
//! (offline guard -> explicit token -> stored credential), the token file
//! read with trim-and-empty validation, and every upstream error string.
//!
//! D7 seam (disclosed in this module's docs): the Node process environment, the stored
//! credential lookup (`ModelRuntime.create` + `getAuth`, upstream
//! `cli/auth-command.ts` `getAuthCredential`) and `AbortSignal` cancellation
//! are embedder-owned; [`RadiusRelayAuthResolver::resolve`] takes them as
//! explicit inputs. Upstream aborts at two `signal?.throwIfAborted()`
//! checkpoints (resolve entry, after the `ModelRuntime` await) plus the
//! explicit-token `readFile` signal; the port models all three surfaces as
//! `cancelled` checks at the same points.

/// Upstream `pi-ai/providers/radius-config.ts` `DEFAULT_RADIUS_GATEWAY`.
pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.pi.dev";

/// Upstream `ENV_RADIUS_GATEWAY`.
pub const ENV_RADIUS_GATEWAY: &str = "PI_RADIUS_GATEWAY";

/// Upstream `core/radius.ts` `RADIUS_MCP_URL` (v1.0.0): MCP endpoint of the
/// gateway the built-in Radius provider signs in to. Upstream computes
/// `` `${normalizeRadiusGatewayUrl(DEFAULT_RADIUS_GATEWAY)}/mcp` `` at module
/// load; the normalization of the vendored default is the constant below.
pub const RADIUS_MCP_URL: &str = "https://radius.pi.dev/mcp";

/// Upstream `core/radius.ts` `RADIUS_PROVIDER_ID`: the id of the built-in
/// Radius login provider.
pub const RADIUS_PROVIDER_ID: &str = "radius";

/// Upstream `pi-ai` `normalizeRadiusGatewayUrl`: default the scheme to https
/// and strip trailing slashes.
pub fn normalize_radius_gateway_url(value: &str) -> String {
    let with_scheme = if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// Upstream `RadiusRelayAuth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadiusRelayAuth {
    pub gateway: String,
    pub token: String,
}

/// Upstream `AuthInput` (`cli/experimental/command-options.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthInput {
    /// `{ type: "token", token }`.
    Token(String),
    /// `{ type: "path", path }` — read the token from a file.
    Path(String),
}

/// Inputs standing in for the Node-runtime-bound collaborators of the
/// upstream resolver (D7 seam).
#[derive(Debug, Clone, Default)]
pub struct ResolveEnvironment<'a> {
    /// Upstream `process.env.PI_OFFLINE !== undefined`.
    pub offline: bool,
    /// Upstream `ModelRuntime.create(...).getAuth("radius", ...)` piped
    /// through `getAuthCredential`; `None` when no stored token exists.
    pub stored_token: Option<&'a str>,
    /// Upstream abort surfaces: the two `options.signal?.throwIfAborted()`
    /// checkpoints plus the explicit-token `readFile` signal, mirrored here
    /// as three checkpoints; `Some(message)` is the abort reason the
    /// embedder owns.
    pub cancelled: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveOptions {
    /// Upstream `options.required`.
    pub required: bool,
}

/// Upstream `RadiusRelayAuthResolver`: resolve explicit or stored Radius
/// credentials anew for every relay connection attempt.
pub struct RadiusRelayAuthResolver {
    input: Option<AuthInput>,
    gateway: String,
}

impl RadiusRelayAuthResolver {
    /// Upstream constructor. `gateway` mirrors the
    /// `process.env[ENV_RADIUS_GATEWAY] ?? DEFAULT_RADIUS_GATEWAY` default
    /// the caller passes in (the default gateway read is environment access,
    /// D7).
    pub fn new(input: Option<AuthInput>, gateway: &str) -> Self {
        Self {
            input,
            gateway: normalize_radius_gateway_url(gateway),
        }
    }

    /// Upstream `get gateway()`.
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// Upstream `resolve(options)`. Returns `Ok(None)` exactly where upstream
    /// resolves `undefined` (no credential and `required: false`).
    pub fn resolve(
        &self,
        options: ResolveOptions,
        environment: ResolveEnvironment<'_>,
        base_dir: &str,
    ) -> Result<Option<RadiusRelayAuth>, String> {
        if let Some(message) = environment.cancelled.clone() {
            return Err(message);
        }
        if environment.offline {
            if options.required {
                return Err("Radius relay connections are unavailable in offline mode".to_string());
            }
            return Ok(None);
        }

        let explicit = self.explicit_token(&environment, base_dir)?;
        if let Some(token) = explicit {
            return Ok(Some(RadiusRelayAuth {
                gateway: self.gateway.clone(),
                token,
            }));
        }

        if let Some(message) = environment.cancelled.clone() {
            return Err(message);
        }
        match environment.stored_token {
            Some(token) if !token.is_empty() => Ok(Some(RadiusRelayAuth {
                gateway: self.gateway.clone(),
                token: token.to_string(),
            })),
            _ => {
                if options.required {
                    return Err(
                        "Radius authentication is required; start Pi and run /login radius, then retry"
                            .to_string(),
                    );
                }
                Ok(None)
            }
        }
    }

    /// Upstream `#explicitToken`.
    fn explicit_token(
        &self,
        environment: &ResolveEnvironment<'_>,
        base_dir: &str,
    ) -> Result<Option<String>, String> {
        let Some(input) = &self.input else {
            return Ok(None);
        };
        if let Some(message) = &environment.cancelled {
            return Err(message.clone());
        }
        let value = match input {
            AuthInput::Token(token) => token.clone(),
            AuthInput::Path(path) => {
                // Upstream: `readFile(resolvePath(path), "utf8")`.
                let resolved = crate::coding_agent::utils::paths::resolve_path(path, base_dir)
                    .map_err(|error| error.to_string())?;
                std::fs::read_to_string(resolved).map_err(|error| error.to_string())?
            }
        };
        let token = value.trim().to_string();
        if token.is_empty() {
            return Err("Radius authentication token must not be empty".to_string());
        }
        Ok(Some(token))
    }
}

#[cfg(test)]
mod tests;
