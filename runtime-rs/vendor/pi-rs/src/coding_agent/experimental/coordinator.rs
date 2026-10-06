//! Port of the deterministic face of upstream `experimental/coordinator.ts`
//! (sha256 c65c9b03ab980d12b4a0bf938b39af7462f40a79d4a4331f94cda9df0ea12b62).
//!
//! Ported: `COORDINATOR_PROTOCOL_VERSION`, the coordinator control-message
//! frame types (`server_registered` / `server_replaced` / `peer_connected` /
//! `peer_disconnected` / `message`), the routed envelope frames
//! (`register_server`, `register_peer`, `send`, `broadcast`, `peer_registered`,
//! `peer_registered.serverConnectionId` inclusion), the
//! `CoordinatorConnectionEvent` surface consumed by the manager, and the exact
//! validation error strings from `registerServer` / `registerPeer` /
//! routed-message handling.
//!
//! `server` and `transport` implement the control router, client connection,
//! public byte proxies and replacement/shutdown lifecycle. Peer iteration is
//! insertion ordered. Blocking IO has independent read/write/close handles.
//! Platform seams remain explicit: Windows uses loopback TCP, not named pipes;
//! Unix socket behavior needs Unix-host validation; process startup and idle
//! timer ownership are supplied by the embedder (see `server` documentation).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod server;
pub mod transport;

/// Upstream `COORDINATOR_PROTOCOL_VERSION`.
pub const COORDINATOR_PROTOCOL_VERSION: u32 = 3;

/// Upstream `CoordinatorMessage` (coordinator -> connection frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorMessage {
    #[serde(rename = "server_registered", rename_all = "camelCase")]
    ServerRegistered {
        server_connection_id: String,
        peers: Vec<String>,
    },
    #[serde(rename = "server_replaced")]
    ServerReplaced,
    #[serde(rename = "peer_connected", rename_all = "camelCase")]
    PeerConnected { peer_id: String },
    #[serde(rename = "peer_disconnected", rename_all = "camelCase")]
    PeerDisconnected { peer_id: String },
    #[serde(rename = "message")]
    Message { from: String, payload: Value },
}

/// Upstream `CoordinatorConnectionEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum CoordinatorConnectionEvent {
    PeerConnected { peer_id: String },
    PeerDisconnected { peer_id: String },
    Message { from: String, payload: Value },
}

/// Upstream `register_server` request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterServerFrame {
    #[serde(rename = "type")]
    pub message_type: String,
    pub protocol: u32,
    pub server_connection_id: String,
    pub endpoint: String,
}

/// Upstream `register_peer` request frame (`serverConnectionId` is included
/// only when a server is already connected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPeerFrame {
    #[serde(rename = "type")]
    pub message_type: String,
    pub protocol: u32,
    pub peer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_connection_id: Option<String>,
}

/// Upstream routed `send` / `broadcast` envelopes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutedEnvelope {
    #[serde(rename = "send", rename_all = "camelCase")]
    Send { to: String, payload: Value },
    #[serde(rename = "broadcast")]
    Broadcast { payload: Value },
}

/// Validate an incoming `register_server` control message, mirroring
/// upstream `registerServer` field checks (protocol first, then
/// serverConnectionId, then endpoint). Returns the accepted fields.
pub fn validate_register_server(frame: &RegisterServerFrame) -> Result<(String, String), String> {
    if frame.protocol != COORDINATOR_PROTOCOL_VERSION {
        return Err("Unsupported coordinator protocol".to_string());
    }
    if frame.server_connection_id.is_empty() {
        return Err("Coordinator serverConnectionId must be a string".to_string());
    }
    if frame.endpoint.is_empty() {
        return Err("Coordinator endpoint must be a string".to_string());
    }
    Ok((frame.server_connection_id.clone(), frame.endpoint.clone()))
}

/// Validate an incoming `register_peer` control message, mirroring upstream
/// `registerPeer`. `existing_peers` carries the coordinator's live peer set;
/// the reserved `"server"` name is always taken.
pub fn validate_register_peer(
    frame: &RegisterPeerFrame,
    existing_peers: &[String],
) -> Result<String, String> {
    if frame.protocol != COORDINATOR_PROTOCOL_VERSION {
        return Err("Unsupported coordinator protocol".to_string());
    }
    if frame.peer_id.is_empty() {
        return Err("Coordinator peerId must be a string".to_string());
    }
    if frame.peer_id == "server" || existing_peers.iter().any(|peer| peer == &frame.peer_id) {
        return Err(format!(
            "Coordinator peer is already connected: {}",
            frame.peer_id
        ));
    }
    Ok(frame.peer_id.clone())
}

/// Upstream `peer_registered` reply construction: `serverConnectionId` is
/// included only when a server is currently connected.
pub fn peer_registered_reply(peer_id: &str, current_server_connection_id: Option<&str>) -> Value {
    match current_server_connection_id {
        Some(server_connection_id) => serde_json::json!({
            "type": "peer_registered",
            "peerId": peer_id,
            "serverConnectionId": server_connection_id,
        }),
        None => serde_json::json!({
            "type": "peer_registered",
            "peerId": peer_id,
        }),
    }
}

/// Upstream routed-message target validation from `handleRoutedMessage`:
/// a `send` target must be a non-empty string.
pub fn validate_send_target(to: &str) -> Result<(), String> {
    if to.is_empty() {
        return Err("Coordinator message target must be a string".to_string());
    }
    Ok(())
}

/// Upstream `server_registered` reply construction.
pub fn server_registered_reply(server_connection_id: &str, peers: &[String]) -> Value {
    serde_json::json!({
        "type": "server_registered",
        "serverConnectionId": server_connection_id,
        "peers": peers,
    })
}

#[cfg(test)]
mod tests;
