//! OAuth metadata/token shapes and the dependency-free structural
//! validators, ported from upstream `packages/mcp/src/oauth/types.ts`
//! (itself adapted from modelcontextprotocol/typescript-sdk v1.29.0).
//!
//! The parsed shapes wrap the raw JSON object: unknown keys survive with
//! their original positions (upstream spreads `{ ...input, <validated
//! fields> }`), `None`-valued validated fields are dropped exactly like the
//! upstream `compact`, and validation error strings are byte-identical.

use serde_json::{Map, Value};

pub type JsonMap = Map<String, Value>;

/// Upstream `object()`: require a JSON object.
pub(crate) fn require_object<'a>(value: &'a Value, name: &str) -> Result<&'a JsonMap, String> {
    value.as_object().ok_or_else(|| format!("Invalid {name}"))
}

/// Apply `{ ...input, key: value }` spread semantics: replace existing keys in
/// position, append new keys at the end in update order.
pub(crate) fn patch_map(input: &JsonMap, updates: Vec<(&str, Option<Value>)>) -> JsonMap {
    let mut patched = input.clone();
    for (key, value) in updates {
        match value {
            Some(value) => {
                patched.insert(key.to_string(), value);
            }
            None => {
                patched.remove(key);
            }
        }
    }
    patched
}

/// Upstream `safeUrl`: a non-empty string parsing as a URL whose scheme is
/// not on the script-scheme blocklist.
pub(crate) fn safe_url(value: &Value, name: &str) -> Result<String, String> {
    let text = required_string(value, name)?;
    let url = url::Url::parse(&text).map_err(|_| format!("Invalid {name}"))?;
    if ["javascript", "data", "vbscript"].contains(&url.scheme()) {
        return Err(format!("Invalid {name}"));
    }
    Ok(text)
}

/// Upstream `requiredString`.
pub(crate) fn required_string(value: &Value, name: &str) -> Result<String, String> {
    match value.as_str() {
        Some(text) if !text.is_empty() => Ok(text.to_string()),
        _ => Err(format!("Invalid {name}")),
    }
}

/// Upstream `absent` (v1.0.0): treats `null` and `""` as absent — servers
/// send them for fields they have no value for, like `scope: ""`. JSON has
/// no `undefined`, so a missing key counts as absent too.
fn absent(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

/// The field is present in the input object (JSON `undefined` does not exist;
/// absence is the upstream `undefined`).
fn field<'a>(input: &'a JsonMap, key: &str) -> Option<&'a Value> {
    input.get(key)
}

/// Upstream `optionalString` (v1.0.0): absent (`undefined`/`null`/`""`)
/// means `undefined`; a present value must be a non-empty string.
fn parse_optional_string(value: Option<&Value>, name: &str) -> Result<Option<String>, String> {
    if absent(value) {
        return Ok(None);
    }
    required_string(value.expect("checked present"), name).map(Some)
}

/// Upstream `optionalStrings` (v1.0.0): `undefined` or `null` means
/// `undefined`; a present value must be an array of strings.
fn parse_strings_optional(
    value: Option<&Value>,
    name: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_strings(value, name).map(Some)
}

/// Upstream `optionalStrings`: absent means `undefined`; a present value must
/// be an array of strings (null throws).
fn parse_strings(value: &Value, name: &str) -> Result<Vec<String>, String> {
    match value.as_array() {
        Some(items) => {
            let mut strings = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(text) => strings.push(text.to_string()),
                    None => return Err(format!("Invalid {name}")),
                }
            }
            Ok(strings)
        }
        None => Err(format!("Invalid {name}")),
    }
}

fn strings_value(strings: &[String]) -> Value {
    Value::Array(
        strings
            .iter()
            .map(|string| Value::from(string.clone()))
            .collect(),
    )
}

/// Upstream `OAuthProtectedResourceMetadata`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthProtectedResourceMetadata {
    raw: JsonMap,
}

impl OAuthProtectedResourceMetadata {
    pub fn resource(&self) -> String {
        self.raw
            .get("resource")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub fn authorization_servers(&self) -> Vec<String> {
        self.raw
            .get("authorization_servers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn scopes_supported(&self) -> Vec<String> {
        self.raw
            .get("scopes_supported")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The full parsed object (validated fields replaced in position).
    pub fn raw(&self) -> &JsonMap {
        &self.raw
    }
}

/// Upstream `parseProtectedResourceMetadata`.
pub fn parse_protected_resource_metadata(
    value: Value,
) -> Result<OAuthProtectedResourceMetadata, String> {
    let input = require_object(&value, "OAuth protected resource metadata")?;
    let mut updates: Vec<(&str, Option<Value>)> = vec![(
        "resource",
        Some(Value::from(safe_url(
            field(input, "resource").unwrap_or(&Value::Null),
            "OAuth protected resource metadata resource",
        )?)),
    )];
    if let Some(raw_servers) = field(input, "authorization_servers") {
        // v1.0.0: a present `null` reads as absent (`optionalStrings`).
        if let Some(servers) = parse_strings_optional(Some(raw_servers), "authorization_servers")? {
            let mut validated = Vec::with_capacity(servers.len());
            for server in &servers {
                validated.push(safe_url(
                    &Value::from(server.clone()),
                    "authorization server URL",
                )?);
            }
            updates.push(("authorization_servers", Some(strings_value(&validated))));
        }
    }
    if let Some(raw_scopes) = field(input, "scopes_supported") {
        if let Some(scopes) = parse_strings_optional(Some(raw_scopes), "scopes_supported")? {
            updates.push(("scopes_supported", Some(strings_value(&scopes))));
        }
    }
    // Upstream `compact` drops validated fields whose value was undefined —
    // impossible for present fields here, so every update is Some.
    let raw = patch_map(input, updates);
    Ok(OAuthProtectedResourceMetadata { raw })
}

/// Upstream `AuthorizationServerMetadata`.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizationServerMetadata {
    raw: JsonMap,
}

impl AuthorizationServerMetadata {
    pub fn issuer(&self) -> String {
        self.string_field("issuer")
    }

    pub fn authorization_endpoint(&self) -> String {
        self.string_field("authorization_endpoint")
    }

    pub fn token_endpoint(&self) -> String {
        self.string_field("token_endpoint")
    }

    pub fn registration_endpoint(&self) -> Option<String> {
        self.optional_string_field("registration_endpoint")
    }

    pub fn scopes_supported(&self) -> Option<Vec<String>> {
        self.strings_field("scopes_supported")
    }

    pub fn response_types_supported(&self) -> Vec<String> {
        self.strings_field("response_types_supported")
            .unwrap_or_default()
    }

    pub fn grant_types_supported(&self) -> Option<Vec<String>> {
        self.strings_field("grant_types_supported")
    }

    pub fn token_endpoint_auth_methods_supported(&self) -> Option<Vec<String>> {
        self.strings_field("token_endpoint_auth_methods_supported")
    }

    pub fn code_challenge_methods_supported(&self) -> Option<Vec<String>> {
        self.strings_field("code_challenge_methods_supported")
    }

    pub fn client_id_metadata_document_supported(&self) -> Option<bool> {
        self.raw
            .get("client_id_metadata_document_supported")
            .and_then(Value::as_bool)
    }

    /// Upstream `authorization_response_iss_parameter_supported` (v1.0.0):
    /// whether authorization responses carry an `iss` parameter (RFC 9207).
    pub fn authorization_response_iss_parameter_supported(&self) -> Option<bool> {
        self.raw
            .get("authorization_response_iss_parameter_supported")
            .and_then(Value::as_bool)
    }

    pub fn raw(&self) -> &JsonMap {
        &self.raw
    }

    fn string_field(&self, key: &str) -> String {
        self.raw
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn optional_string_field(&self, key: &str) -> Option<String> {
        self.raw
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn strings_field(&self, key: &str) -> Option<Vec<String>> {
        self.raw.get(key).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
    }
}

/// Upstream `parseAuthorizationServerMetadata`.
pub fn parse_authorization_server_metadata(
    value: Value,
) -> Result<AuthorizationServerMetadata, String> {
    let input = require_object(&value, "authorization server metadata")?;
    let response_types = parse_strings(
        field(input, "response_types_supported").unwrap_or(&Value::Null),
        "response_types_supported",
    )
    .map_err(|_| "Invalid response_types_supported".to_string())?;
    let mut updates: Vec<(&str, Option<Value>)> = vec![
        (
            "issuer",
            Some(Value::from(safe_url(
                field(input, "issuer").unwrap_or(&Value::Null),
                "authorization server issuer",
            )?)),
        ),
        (
            "authorization_endpoint",
            Some(Value::from(safe_url(
                field(input, "authorization_endpoint").unwrap_or(&Value::Null),
                "authorization endpoint",
            )?)),
        ),
        (
            "token_endpoint",
            Some(Value::from(safe_url(
                field(input, "token_endpoint").unwrap_or(&Value::Null),
                "token endpoint",
            )?)),
        ),
    ];
    // Upstream `optionalUrl` (v1.0.0): absent (`undefined`/`null`/`""`)
    // drops the field; otherwise the URL must validate.
    if !absent(field(input, "registration_endpoint")) {
        let endpoint = safe_url(
            field(input, "registration_endpoint").expect("checked present"),
            "registration endpoint",
        )?;
        updates.push(("registration_endpoint", Some(Value::from(endpoint))));
    }
    if let Some(raw_scopes) = field(input, "scopes_supported") {
        if let Some(scopes) = parse_strings_optional(Some(raw_scopes), "scopes_supported")? {
            updates.push(("scopes_supported", Some(strings_value(&scopes))));
        }
    }
    updates.push((
        "response_types_supported",
        Some(strings_value(&response_types)),
    ));
    if let Some(raw_grants) = field(input, "grant_types_supported") {
        if let Some(grants) = parse_strings_optional(Some(raw_grants), "grant_types_supported")? {
            updates.push(("grant_types_supported", Some(strings_value(&grants))));
        }
    }
    if let Some(raw_methods) = field(input, "token_endpoint_auth_methods_supported") {
        if let Some(methods) =
            parse_strings_optional(Some(raw_methods), "token_endpoint_auth_methods_supported")?
        {
            updates.push((
                "token_endpoint_auth_methods_supported",
                Some(strings_value(&methods)),
            ));
        }
    }
    if let Some(raw_challenges) = field(input, "code_challenge_methods_supported") {
        if let Some(challenges) =
            parse_strings_optional(Some(raw_challenges), "code_challenge_methods_supported")?
        {
            updates.push((
                "code_challenge_methods_supported",
                Some(strings_value(&challenges)),
            ));
        }
    }
    if let Some(raw_flag) = field(input, "client_id_metadata_document_supported") {
        if raw_flag.is_boolean() {
            updates.push((
                "client_id_metadata_document_supported",
                Some(raw_flag.clone()),
            ));
        }
    }
    // v1.0.0: whether authorization responses carry an `iss` parameter
    // (RFC 9207).
    if let Some(raw_flag) = field(input, "authorization_response_iss_parameter_supported") {
        if raw_flag.is_boolean() {
            updates.push((
                "authorization_response_iss_parameter_supported",
                Some(raw_flag.clone()),
            ));
        }
    }
    // Upstream `compact` drops optional validated fields whose parsed value
    // was undefined: present-but-invalid values already threw above.
    let raw = patch_map(input, updates);
    Ok(AuthorizationServerMetadata { raw })
}

/// Upstream `OAuthTokens`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthTokens {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<f64>,
    pub scope: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
}

impl OAuthTokens {
    /// The exact serialized shape of the upstream `compact({...})` result:
    /// fixed key order, absent optional keys.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "access_token".into(),
            Value::from(self.access_token.clone()),
        );
        object.insert("token_type".into(), Value::from(self.token_type.clone()));
        if let Some(expires_in) = self.expires_in {
            object.insert("expires_in".into(), number_value(expires_in));
        }
        if let Some(scope) = &self.scope {
            object.insert("scope".into(), Value::from(scope.clone()));
        }
        if let Some(refresh_token) = &self.refresh_token {
            object.insert("refresh_token".into(), Value::from(refresh_token.clone()));
        }
        if let Some(id_token) = &self.id_token {
            object.insert("id_token".into(), Value::from(id_token.clone()));
        }
        Value::Object(object)
    }
}

/// JS numbers: integral values serialize without a fractional part
/// (`JSON.stringify(3600.0) === "3600"`).
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() <= 9.007_199_254_740_992e15 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Upstream `Number(value)` for `expires_in`: strings parse, null becomes 0,
/// everything else goes through the number coercion (NaN fails the finite
/// check).
fn coerce_number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::Null => 0.0,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::String(text) => text.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// Upstream `parseOAuthTokens`.
pub fn parse_oauth_tokens(value: Value) -> Result<OAuthTokens, String> {
    let input = require_object(&value, "OAuth token response")?;
    // v1.0.0: absent (`undefined`/`null`/`""`) means no expiry —
    // `Number(null)` is 0, which would mark the token as expired at once.
    let expires = if absent(field(input, "expires_in")) {
        None
    } else {
        Some(coerce_number(
            field(input, "expires_in").expect("checked present"),
        ))
    };
    if expires.is_some_and(|expires| !expires.is_finite()) {
        return Err("Invalid expires_in".to_string());
    }
    Ok(OAuthTokens {
        access_token: required_string(
            field(input, "access_token").unwrap_or(&Value::Null),
            "access_token",
        )?,
        token_type: required_string(
            field(input, "token_type").unwrap_or(&Value::Null),
            "token_type",
        )?,
        expires_in: expires,
        scope: parse_optional_string(field(input, "scope"), "scope")?,
        refresh_token: parse_optional_string(field(input, "refresh_token"), "refresh_token")?,
        id_token: parse_optional_string(field(input, "id_token"), "id_token")?,
    })
}

/// Upstream `OAuthClientMetadata`: the registration request body. Modeled as
/// a raw object (insertion order is preserved into the registration JSON),
/// with the provider constructor applying the upstream spread defaults.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OAuthClientMetadata {
    pub raw: JsonMap,
}

impl OAuthClientMetadata {
    pub fn from_raw(raw: JsonMap) -> Self {
        OAuthClientMetadata { raw }
    }

    pub fn redirect_uris(&self) -> Vec<String> {
        self.raw
            .get("redirect_uris")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn scope(&self) -> Option<String> {
        self.raw
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Upstream `OAuthClientInformationMixed`: registered client details (either
/// bare credentials or a full registration response).
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientInformation {
    raw: JsonMap,
}

/// Upstream `OAuthClientInformationMixed` (the `OAuthClientInformation |
/// OAuthClientInformationFull` union): the port's parsed carrier is the same
/// raw object for both union members.
pub type OAuthClientInformationMixed = OAuthClientInformation;

impl OAuthClientInformation {
    pub fn from_raw(raw: JsonMap) -> Self {
        OAuthClientInformation { raw }
    }

    pub fn client_id(&self) -> String {
        self.raw
            .get("client_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub fn client_secret(&self) -> Option<String> {
        self.raw
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Upstream `"token_endpoint_auth_method" in information`.
    pub fn token_endpoint_auth_method(&self) -> Option<String> {
        self.raw
            .get("token_endpoint_auth_method")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub fn raw(&self) -> &JsonMap {
        &self.raw
    }
}

/// Upstream `parseClientInformation`.
pub fn parse_client_information(value: Value) -> Result<OAuthClientInformation, String> {
    let input = require_object(&value, "OAuth client registration response")?;
    let mut updates: Vec<(&str, Option<Value>)> = vec![(
        "client_id",
        Some(Value::from(required_string(
            field(input, "client_id").unwrap_or(&Value::Null),
            "client_id",
        )?)),
    )];
    // Upstream `compact` drops the key when the validated value is
    // undefined — including a present-but-absent `null`/`""` (v1.0.0).
    let secret = parse_optional_string(field(input, "client_secret"), "client_secret")?;
    updates.push(("client_secret", secret.map(Value::from)));
    for key in ["client_id_issued_at", "client_secret_expires_at"] {
        if let Some(raw) = field(input, key) {
            // `typeof input[key] === "number" ? input[key] : undefined`.
            let number = if raw.is_number() {
                Some(raw.clone())
            } else {
                None
            };
            updates.push((key, number));
        }
    }
    // `optionalStrings(...) ?? []` (v1.0.0): absent or `null` becomes an
    // empty array.
    let redirect_uris =
        parse_strings_optional(field(input, "redirect_uris"), "redirect_uris")?.unwrap_or_default();
    updates.push(("redirect_uris", Some(strings_value(&redirect_uris))));
    let raw = patch_map(input, updates);
    Ok(OAuthClientInformation { raw })
}

/// Upstream `OAuthDiscoveryState` (persisted discovery results).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OAuthDiscoveryState {
    pub authorization_server_url: String,
    pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
    pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
    pub resource_metadata_url: Option<String>,
}

impl OAuthDiscoveryState {
    /// The stored object shape (key order per the upstream construction
    /// site in `runFlow`: authorizationServerUrl first, then
    /// authorizationServerMetadata, resourceMetadata, resourceMetadataUrl as
    /// present).
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "authorizationServerUrl".into(),
            Value::from(self.authorization_server_url.clone()),
        );
        if let Some(metadata) = &self.authorization_server_metadata {
            object.insert(
                "authorizationServerMetadata".into(),
                Value::Object(metadata.raw().clone()),
            );
        }
        if let Some(metadata) = &self.resource_metadata {
            object.insert(
                "resourceMetadata".into(),
                Value::Object(metadata.raw().clone()),
            );
        }
        if let Some(url) = &self.resource_metadata_url {
            object.insert("resourceMetadataUrl".into(), Value::from(url.clone()));
        }
        Value::Object(object)
    }

    pub fn from_value(value: &Value) -> Option<OAuthDiscoveryState> {
        let object = value.as_object()?;
        let authorization_server_url = object
            .get("authorizationServerUrl")
            .and_then(Value::as_str)
            .map(str::to_string)?;
        Some(OAuthDiscoveryState {
            authorization_server_url,
            authorization_server_metadata: object.get("authorizationServerMetadata").and_then(
                |metadata| {
                    parse_authorization_server_metadata(Value::Object(
                        metadata.as_object()?.clone(),
                    ))
                    .ok()
                },
            ),
            resource_metadata: object.get("resourceMetadata").and_then(|metadata| {
                parse_protected_resource_metadata(Value::Object(metadata.as_object()?.clone())).ok()
            }),
            resource_metadata_url: object
                .get("resourceMetadataUrl")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// Upstream `OAuthServerInfo` (the in-flow discovery result).
#[derive(Debug, Clone, Default)]
pub struct OAuthServerInfo {
    pub authorization_server_url: String,
    pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
    pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
}

/// Upstream `OAuthChallenge` (a parsed bearer WWW-Authenticate challenge).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OAuthChallenge {
    /// Upstream keeps this as a `URL` object; the port stores the parsed URL.
    pub resource_metadata_url: Option<url::Url>,
    pub scope: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}
