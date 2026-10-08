//! Protocol message surface. Port of `packages/protocol/src/protocol.ts`:
//! version constant, envelope types, the UUIDv4 server-id guard, and the
//! typed-to-CBOR encoding in schema field order.
//!
//! Upstream derives these types from TypeBox schemas whose runtime `Check`
//! enforces literal tags, strict objects (no additional properties), string
//! minLength/pattern, and integer minimums. The port keeps the types loose
//! (plain `String` fields) and exposes the same runtime validation through
//! [`crate::protocol::codec::parse_client_message`],
//! [`crate::protocol::codec::parse_server_message`], and the `validate_*`
//! functions, which run on both the decode and encode paths exactly like
//! upstream's `encodeProtocolMessage` re-parse (divergence D8: the two
//! response object variants are one struct with [`ResponseOutcome`]).

use crate::protocol::cbor::{json_value_to_cbor, CborValue};
use crate::protocol::json::JsonValue;

/// Protocol version negotiated in the hello exchange (`PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: u64 = 8;

/// Upstream `ProtocolErrorCode = string`.
pub type ProtocolErrorCode = String;

/// Upstream `ServerId = Static<ServerIdSchema>`; the canonical UUIDv4 pattern
/// is enforced by [`is_server_id`], not by the type.
pub type ServerId = String;

/// Canonical UUIDv4 check (upstream `isServerId`, the TypeBox `pattern`
/// `^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`).
pub fn is_server_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let valid = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            14 => *byte == b'4',
            19 => matches!(*byte, b'8' | b'9' | b'a' | b'b'),
            _ => matches!(*byte, b'0'..=b'9' | b'a'..=b'f'),
        };
        if !valid {
            return false;
        }
    }
    true
}

/// Upstream `ProtocolError` (`{code, message}` with `minLength: 1` code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

/// Must be the first frame sent by a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    pub version: u64,
}

/// A session call, fenced to one logical server, durable session, and live
/// attachment (upstream `SessionTarget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTarget {
    pub server_id: String,
    pub session_id: String,
    pub attachment_id: String,
}

/// A server-wide call target, fenced to one logical server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTarget {
    pub server_id: String,
}

/// Upstream `RpcTarget = ServerTarget | SessionTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcTarget {
    Server(ServerTarget),
    Session(SessionTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestEnvelope {
    pub id: String,
    pub target: RpcTarget,
    /// Opaque strict-JSON payload; its meaning belongs to Chord, not the
    /// protocol.
    pub call: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelEnvelope {
    pub id: String,
    pub target: RpcTarget,
}

/// Upstream `ClientMessage = ClientHello | RequestEnvelope | CancelEnvelope`.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Hello(ClientHello),
    Request(RequestEnvelope),
    Cancel(CancelEnvelope),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHello {
    pub server_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHelloError {
    pub error: ProtocolError,
}

/// The two upstream response object variants: `ok: true` with an optional
/// `result`, or `ok: false` with a required `error` (divergence D8).
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseOutcome {
    Success {
        /// Absent for void results; any strict-JSON value otherwise.
        result: Option<JsonValue>,
    },
    Failure {
        error: ProtocolError,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseEnvelope {
    pub id: String,
    pub outcome: ResponseOutcome,
}

/// Out-of-band update for a service subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceEventEnvelope {
    pub subscription_id: String,
    pub update: JsonValue,
}

/// Out-of-band update to this presentation's selected Session route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentEnvelope {
    /// The attached session target, or `None` when detached.
    pub attachment: Option<SessionTarget>,
}

/// Upstream `ServerMessage = ServerHello | ServerHelloError | ResponseEnvelope
/// | ServiceEventEnvelope | AttachmentEnvelope`.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Hello(ServerHello),
    HelloError(ServerHelloError),
    Response(ResponseEnvelope),
    ServiceUpdate(ServiceEventEnvelope),
    Attachment(AttachmentEnvelope),
}

fn text(value: &str) -> CborValue {
    CborValue::Text(value.to_string())
}

impl ProtocolError {
    pub(crate) fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            ("code".to_string(), text(&self.code)),
            ("message".to_string(), text(&self.message)),
        ])
    }
}

impl ServerTarget {
    pub(crate) fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![("serverId".to_string(), text(&self.server_id))])
    }
}

impl SessionTarget {
    pub(crate) fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            ("serverId".to_string(), text(&self.server_id)),
            ("sessionId".to_string(), text(&self.session_id)),
            ("attachmentId".to_string(), text(&self.attachment_id)),
        ])
    }
}

impl RpcTarget {
    pub(crate) fn to_cbor(&self) -> CborValue {
        match self {
            RpcTarget::Server(target) => target.to_cbor(),
            RpcTarget::Session(target) => target.to_cbor(),
        }
    }
}

impl ClientMessage {
    /// Encodes into the CBOR tree in schema field order; wire bytes are
    /// pinned against the node oracle in the codec tests.
    pub fn to_cbor(&self) -> CborValue {
        match self {
            ClientMessage::Hello(hello) => CborValue::Map(vec![
                ("type".to_string(), text("hello")),
                ("version".to_string(), CborValue::Uint(hello.version)),
            ]),
            ClientMessage::Request(request) => CborValue::Map(vec![
                ("type".to_string(), text("request")),
                ("id".to_string(), text(&request.id)),
                ("target".to_string(), request.target.to_cbor()),
                ("call".to_string(), json_value_to_cbor(&request.call)),
            ]),
            ClientMessage::Cancel(cancel) => CborValue::Map(vec![
                ("type".to_string(), text("cancel")),
                ("id".to_string(), text(&cancel.id)),
                ("target".to_string(), cancel.target.to_cbor()),
            ]),
        }
    }
}

impl ServerMessage {
    /// Encodes into the CBOR tree in schema field order; the optional void
    /// `result` is omitted exactly like upstream's `Type.Optional`.
    pub fn to_cbor(&self) -> CborValue {
        match self {
            ServerMessage::Hello(hello) => CborValue::Map(vec![
                ("type".to_string(), text("hello")),
                ("version".to_string(), CborValue::Uint(PROTOCOL_VERSION)),
                ("serverId".to_string(), text(&hello.server_id)),
            ]),
            ServerMessage::HelloError(error) => CborValue::Map(vec![
                ("type".to_string(), text("hello_error")),
                ("error".to_string(), error.error.to_cbor()),
            ]),
            ServerMessage::Response(response) => {
                let mut entries = vec![
                    ("type".to_string(), text("response")),
                    ("id".to_string(), text(&response.id)),
                ];
                match &response.outcome {
                    ResponseOutcome::Success { result } => {
                        entries.push(("ok".to_string(), CborValue::Bool(true)));
                        if let Some(result) = result {
                            entries.push(("result".to_string(), json_value_to_cbor(result)));
                        }
                    }
                    ResponseOutcome::Failure { error } => {
                        entries.push(("ok".to_string(), CborValue::Bool(false)));
                        entries.push(("error".to_string(), error.to_cbor()));
                    }
                }
                CborValue::Map(entries)
            }
            ServerMessage::ServiceUpdate(event) => CborValue::Map(vec![
                ("type".to_string(), text("service_update")),
                ("subscriptionId".to_string(), text(&event.subscription_id)),
                ("update".to_string(), json_value_to_cbor(&event.update)),
            ]),
            ServerMessage::Attachment(attachment) => CborValue::Map(vec![
                ("type".to_string(), text("attachment")),
                (
                    "attachment".to_string(),
                    match &attachment.attachment {
                        Some(target) => target.to_cbor(),
                        None => CborValue::Null,
                    },
                ),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_eight() {
        assert_eq!(PROTOCOL_VERSION, 8);
    }

    #[test]
    fn accepts_canonical_uuidv4_server_ids() {
        assert!(is_server_id("00000000-0000-4000-8000-000000000001"));
        assert!(is_server_id("f81d4fae-7dec-4d0f-a16a-9ad1a6183cbd"));
        assert!(is_server_id("ffffffff-ffff-4fff-bfff-ffffffffffff"));
    }

    #[test]
    fn rejects_non_canonical_server_ids() {
        // Cases from the upstream protocol.test.ts table.
        assert!(!is_server_id(""));
        assert!(!is_server_id("server-1"));
        assert!(!is_server_id("00000000-0000-7000-8000-000000000001"));
        assert!(!is_server_id("00000000-0000-4000-7000-000000000001"));
        assert!(!is_server_id("00000000-0000-4000-8000-00000000000A"));
        assert!(!is_server_id("00000000-0000-4000-8000-000000000001 "));
        assert!(!is_server_id("000000000000400080000000000000001"));
    }

    #[test]
    fn encodes_messages_in_schema_field_order() {
        // Expected wires are the node oracle's payload hex for the same
        // plain objects (tests/fixtures/protocol_oracle/oracle.out.txt).
        let server_id = "00000000-0000-4000-8000-000000000001";
        let hex = |value: &CborValue| -> String {
            crate::protocol::cbor::encode_cbor(value, crate::protocol::cbor::CborOptions::new())
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let messages: Vec<(CborValue, &str)> = vec![
            (
                ClientMessage::Hello(ClientHello { version: 8 }).to_cbor(),
                "a264747970656568656c6c6f6776657273696f6e08",
            ),
            (
                ClientMessage::Hello(ClientHello { version: 0 }).to_cbor(),
                "a264747970656568656c6c6f6776657273696f6e00",
            ),
            (
                ClientMessage::Hello(ClientHello { version: 9 }).to_cbor(),
                "a264747970656568656c6c6f6776657273696f6e09",
            ),
            (
                ClientMessage::Request(RequestEnvelope {
                    id: "request-1".into(),
                    target: RpcTarget::Server(ServerTarget {
                        server_id: server_id.into(),
                    }),
                    call: JsonValue::object(vec![
                        ("serviceId".into(), JsonValue::string("pi.models")),
                        ("member".into(), JsonValue::string("list")),
                        ("args".into(), JsonValue::Array(vec![])),
                    ]),
                })
                .to_cbor(),
                "a46474797065677265717565737462696469726571756573742d3166746172676574a1687365727665724964782430303030303030302d303030302d343030302d383030302d3030303030303030303030316463616c6ca3697365727669636549646970692e6d6f64656c73666d656d626572646c697374646172677380",
            ),
            (
                ClientMessage::Cancel(CancelEnvelope {
                    id: "request-1".into(),
                    target: RpcTarget::Server(ServerTarget {
                        server_id: server_id.into(),
                    }),
                })
                .to_cbor(),
                "a364747970656663616e63656c62696469726571756573742d3166746172676574a1687365727665724964782430303030303030302d303030302d343030302d383030302d303030303030303030303031",
            ),
            (
                ServerMessage::Hello(ServerHello {
                    server_id: server_id.into(),
                })
                .to_cbor(),
                "a364747970656568656c6c6f6776657273696f6e08687365727665724964782430303030303030302d303030302d343030302d383030302d303030303030303030303031",
            ),
            (
                ServerMessage::HelloError(ServerHelloError {
                    error: ProtocolError {
                        code: "wrong_server".into(),
                        message: "safe".into(),
                    },
                })
                .to_cbor(),
                "a264747970656b68656c6c6f5f6572726f72656572726f72a264636f64656c77726f6e675f736572766572676d6573736167656473616665",
            ),
            (
                ServerMessage::Response(ResponseEnvelope {
                    id: "request-1".into(),
                    outcome: ResponseOutcome::Success { result: None },
                })
                .to_cbor(),
                "a3647479706568726573706f6e736562696469726571756573742d31626f6bf5",
            ),
            (
                ServerMessage::Response(ResponseEnvelope {
                    id: "request-1".into(),
                    outcome: ResponseOutcome::Success {
                        result: Some(JsonValue::Array(vec![])),
                    },
                })
                .to_cbor(),
                "a4647479706568726573706f6e736562696469726571756573742d31626f6bf566726573756c7480",
            ),
            (
                ServerMessage::Response(ResponseEnvelope {
                    id: "request-1".into(),
                    outcome: ResponseOutcome::Failure {
                        error: ProtocolError {
                            code: "service_not_found".into(),
                            message: "safe".into(),
                        },
                    },
                })
                .to_cbor(),
                "a4647479706568726573706f6e736562696469726571756573742d31626f6bf4656572726f72a264636f646571736572766963655f6e6f745f666f756e64676d6573736167656473616665",
            ),
            (
                ServerMessage::ServiceUpdate(ServiceEventEnvelope {
                    subscription_id: "subscription-1".into(),
                    update: JsonValue::object(vec![(
                        "applicationDefined".into(),
                        JsonValue::Bool(true),
                    )]),
                })
                .to_cbor(),
                "a364747970656e736572766963655f7570646174656e737562736372697074696f6e49646e737562736372697074696f6e2d3166757064617465a1726170706c69636174696f6e446566696e6564f5",
            ),
            (
                ServerMessage::Attachment(AttachmentEnvelope {
                    attachment: Some(SessionTarget {
                        server_id: server_id.into(),
                        session_id: "session-1".into(),
                        attachment_id: "attachment-1".into(),
                    }),
                })
                .to_cbor(),
                "a264747970656a6174746163686d656e746a6174746163686d656e74a3687365727665724964782430303030303030302d303030302d343030302d383030302d3030303030303030303030316973657373696f6e49646973657373696f6e2d316c6174746163686d656e7449646c6174746163686d656e742d31",
            ),
            (
                ServerMessage::Attachment(AttachmentEnvelope { attachment: None }).to_cbor(),
                "a264747970656a6174746163686d656e746a6174746163686d656e74f6",
            ),
        ];
        for (value, wire) in &messages {
            assert_eq!(&hex(value), wire);
        }
    }
}
