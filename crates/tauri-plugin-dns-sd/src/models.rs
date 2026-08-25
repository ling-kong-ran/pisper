use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A TXT attribute value as supplied by callers when advertising.
///
/// Mirrors the shared `TxtValue` contract (RFC 6763 §6):
/// - `true`  — bare key, present with no value.
/// - `null`  — key present with an empty value (`key=`).
/// - bytes   — key with a binary value.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TxtRecordValue {
    BooleanFlag(bool),
    BinaryData(Vec<u8>),
    Null,
}

/// A TXT attribute value as emitted back to callers in a [`ServiceRecord`].
///
/// Serializes untagged so the guest-js layer receives exactly one of `true`,
/// `null`, or an array of byte values — matching the shared `TxtValue` type.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum TxtWireValue {
    /// Bare key present with no value → `true`.
    Flag(bool),
    /// Key with a binary value → array of bytes.
    Bytes(Vec<u8>),
    /// Key present with an empty value (`key=`) → `null`.
    Null,
}

// Browse options
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseOptions {
    pub service: BrowseServiceSpec, // Required - no wildcard browsing
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseServiceSpec {
    #[serde(rename = "type")]
    pub type_name: String,
    pub protocol: TransportProtocol,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub subtypes: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    Tcp,
    Udp,
}

impl TransportProtocol {
    pub const fn dns_label(self) -> &'static str {
        match self {
            Self::Tcp => "_tcp",
            Self::Udp => "_udp",
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEventPayload {
    pub browse_id: u64,
    pub service: ServiceRecord,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BrowseStoppedPayload {
    pub browse_id: u64,
    pub reason: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRecord {
    pub name: String,
    pub full_name: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub service_type: String,
    pub protocol: String,
    pub domain: String,
    pub subtypes: Vec<String>,
    pub addresses: Vec<String>,
    pub txt: HashMap<String, TxtWireValue>,
    pub is_active: bool,
    pub last_seen_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BrowseHandle {
    pub browse_id: u64,
}

// Advertise
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertiseOptions {
    pub service: AdvertiseServiceSpec,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertiseServiceSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub protocol: TransportProtocol,
    pub port: u16,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub subtypes: Vec<String>,
    #[serde(default)]
    pub txt: HashMap<String, TxtRecordValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AdvertiseHandle {
    pub advertise_id: u64,
    /// The final instance name in use (may differ from the requested name if the
    /// OS resolved a conflict by renaming).
    pub name: String,
}
