//! Validated message codec. Port of `packages/protocol/src/codec.ts` plus the
//! runtime `Check` halves of `packages/protocol/src/protocol.ts` (TypeBox
//! schema validation), which in Rust are the `parse_*` / `validate_*`
//! functions below.
//!
//! Error strings, accept/reject sets, decoder stickiness after failure, and
//! wire bytes are pinned against upstream tests and the node oracle
//! (`tests/fixtures/protocol_oracle/`). Non-JSON payloads (byte strings, non-finite
//! floats) are rejected during CBOR-to-JSON conversion with the identical
//! "Invalid client/server protocol message" text (divergence D5).

use crate::protocol::cbor::{
    cbor_value_to_json, decode_cbor, encode_cbor, CborFailure, CborOptions,
};
use crate::protocol::framing::{
    encode_frame, FrameDecoder, FrameDecoderOptions, FrameError, DEFAULT_MAX_FRAME_LENGTH,
};
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    is_server_id, AttachmentEnvelope, CancelEnvelope, ClientHello, ClientMessage, ProtocolError,
    RequestEnvelope, ResponseEnvelope, ResponseOutcome, RpcTarget, ServerHello, ServerHelloError,
    ServerMessage, ServerTarget, ServiceEventEnvelope, SessionTarget, PROTOCOL_VERSION,
};

/// Port of upstream `ProtocolValidationError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolValidationError {
    message: String,
}

impl ProtocolValidationError {
    pub fn new(message: impl Into<String>) -> ProtocolValidationError {
        ProtocolValidationError {
            message: message.into(),
        }
    }

    /// The exact upstream `error.message` text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ProtocolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtocolValidationError {}

/// `Number.isInteger(version) && version === PROTOCOL_VERSION`.
pub fn is_supported_protocol_version(version: f64) -> bool {
    version.fract() == 0.0 && version == PROTOCOL_VERSION as f64
}

fn invalid_client() -> ProtocolValidationError {
    ProtocolValidationError::new("Invalid client protocol message")
}

fn invalid_server() -> ProtocolValidationError {
    ProtocolValidationError::new("Invalid server protocol message")
}

fn non_empty_id(entries: &[(String, JsonValue)], key: &str) -> Option<String> {
    entries
        .iter()
        .find(|(entry_key, _)| entry_key == key)
        .and_then(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn exact_keys(entries: &[(String, JsonValue)], keys: &[&str]) -> bool {
    entries.len() == keys.len() && entries.iter().all(|(key, _)| keys.contains(&key.as_str()))
}

fn at_most_keys(entries: &[(String, JsonValue)], keys: &[&str]) -> bool {
    entries.iter().all(|(key, _)| keys.contains(&key.as_str()))
}

/// Type.Integer view of a JSON number: integral values only, widened like JS
/// numbers (an integral float64 counts as an integer).
fn integer_field(entries: &[(String, JsonValue)], key: &str) -> Option<i128> {
    entries
        .iter()
        .find(|(entry_key, _)| entry_key == key)
        .and_then(|(_, value)| value.as_number())
        .and_then(|number| number.as_integer())
}

fn parse_session_target(value: &JsonValue) -> Option<SessionTarget> {
    let entries = value.as_object()?;
    if !exact_keys(entries, &["serverId", "sessionId", "attachmentId"]) {
        return None;
    }
    let server_id = non_empty_id(entries, "serverId")?;
    if !is_server_id(&server_id) {
        return None;
    }
    Some(SessionTarget {
        server_id,
        session_id: non_empty_id(entries, "sessionId")?,
        attachment_id: non_empty_id(entries, "attachmentId")?,
    })
}

fn parse_server_target(value: &JsonValue) -> Option<ServerTarget> {
    let entries = value.as_object()?;
    if !exact_keys(entries, &["serverId"]) {
        return None;
    }
    let server_id = non_empty_id(entries, "serverId")?;
    if !is_server_id(&server_id) {
        return None;
    }
    Some(ServerTarget { server_id })
}

fn parse_rpc_target(value: &JsonValue) -> Option<RpcTarget> {
    match parse_server_target(value) {
        Some(target) => Some(RpcTarget::Server(target)),
        None => parse_session_target(value).map(RpcTarget::Session),
    }
}

fn parse_protocol_error(value: &JsonValue) -> Option<ProtocolError> {
    let entries = value.as_object()?;
    if !exact_keys(entries, &["code", "message"]) {
        return None;
    }
    Some(ProtocolError {
        code: non_empty_id(entries, "code")?,
        message: entries
            .iter()
            .find(|(key, _)| key == "message")
            .and_then(|(_, value)| value.as_str())?
            .to_string(),
    })
}

/// Runtime check of `ClientMessageSchema` plus `isJsonValue`, returning the
/// typed message. TypeBox semantics: literal `type` tag, strict objects, and
/// `version: Type.Integer({minimum: 0})` — any integer >= 0 is accepted for
/// negotiation.
pub fn parse_client_message(value: &JsonValue) -> Result<ClientMessage, ProtocolValidationError> {
    let entries = value.as_object().ok_or_else(invalid_client)?;
    let message_type = entries
        .iter()
        .find(|(key, _)| key == "type")
        .and_then(|(_, value)| value.as_str())
        .ok_or_else(invalid_client)?;
    match message_type {
        "hello" => {
            if !exact_keys(entries, &["type", "version"]) {
                return Err(invalid_client());
            }
            let version = integer_field(entries, "version").filter(|version| *version >= 0);
            let version = version.ok_or_else(invalid_client)?;
            let version = u64::try_from(version).map_err(|_| invalid_client())?;
            Ok(ClientMessage::Hello(ClientHello { version }))
        }
        "request" => {
            if !exact_keys(entries, &["type", "id", "target", "call"]) {
                return Err(invalid_client());
            }
            let id = non_empty_id(entries, "id").ok_or_else(invalid_client)?;
            let target = entries
                .iter()
                .find(|(key, _)| key == "target")
                .map(|(_, value)| value)
                .and_then(parse_rpc_target)
                .ok_or_else(invalid_client)?;
            let call = entries
                .iter()
                .find(|(key, _)| key == "call")
                .map(|(_, value)| value.clone())
                .ok_or_else(invalid_client)?;
            Ok(ClientMessage::Request(RequestEnvelope { id, target, call }))
        }
        "cancel" => {
            if !exact_keys(entries, &["type", "id", "target"]) {
                return Err(invalid_client());
            }
            let id = non_empty_id(entries, "id").ok_or_else(invalid_client)?;
            let target = entries
                .iter()
                .find(|(key, _)| key == "target")
                .map(|(_, value)| value)
                .and_then(parse_rpc_target)
                .ok_or_else(invalid_client)?;
            Ok(ClientMessage::Cancel(CancelEnvelope { id, target }))
        }
        _ => Err(invalid_client()),
    }
}

/// Runtime check of `ServerMessageSchema` plus `isJsonValue`, returning the
/// typed message.
pub fn parse_server_message(value: &JsonValue) -> Result<ServerMessage, ProtocolValidationError> {
    let entries = value.as_object().ok_or_else(invalid_server)?;
    let message_type = entries
        .iter()
        .find(|(key, _)| key == "type")
        .and_then(|(_, value)| value.as_str())
        .ok_or_else(invalid_server)?;
    match message_type {
        "hello" => {
            if !exact_keys(entries, &["type", "version", "serverId"]) {
                return Err(invalid_server());
            }
            if integer_field(entries, "version") != Some(i128::from(PROTOCOL_VERSION)) {
                return Err(invalid_server());
            }
            let server_id = non_empty_id(entries, "serverId").ok_or_else(invalid_server)?;
            if !is_server_id(&server_id) {
                return Err(invalid_server());
            }
            Ok(ServerMessage::Hello(ServerHello { server_id }))
        }
        "hello_error" => {
            if !exact_keys(entries, &["type", "error"]) {
                return Err(invalid_server());
            }
            let error = entries
                .iter()
                .find(|(key, _)| key == "error")
                .map(|(_, value)| value)
                .and_then(parse_protocol_error)
                .ok_or_else(invalid_server)?;
            Ok(ServerMessage::HelloError(ServerHelloError { error }))
        }
        "response" => {
            if !at_most_keys(entries, &["type", "id", "ok", "result", "error"]) {
                return Err(invalid_server());
            }
            let id = non_empty_id(entries, "id").ok_or_else(invalid_server)?;
            match entries.iter().find(|(key, _)| key == "ok") {
                Some((_, JsonValue::Bool(true))) => {
                    // `result: Type.Optional(...)`; `error` must not appear.
                    if !at_most_keys(entries, &["type", "id", "ok", "result"]) {
                        return Err(invalid_server());
                    }
                    let result = entries
                        .iter()
                        .find(|(key, _)| key == "result")
                        .map(|(_, value)| value.clone());
                    Ok(ServerMessage::Response(ResponseEnvelope {
                        id,
                        outcome: ResponseOutcome::Success { result },
                    }))
                }
                Some((_, JsonValue::Bool(false))) => {
                    if !exact_keys(entries, &["type", "id", "ok", "error"]) {
                        return Err(invalid_server());
                    }
                    let error = entries
                        .iter()
                        .find(|(key, _)| key == "error")
                        .map(|(_, value)| value)
                        .and_then(parse_protocol_error)
                        .ok_or_else(invalid_server)?;
                    Ok(ServerMessage::Response(ResponseEnvelope {
                        id,
                        outcome: ResponseOutcome::Failure { error },
                    }))
                }
                _ => Err(invalid_server()),
            }
        }
        "service_update" => {
            if !exact_keys(entries, &["type", "subscriptionId", "update"]) {
                return Err(invalid_server());
            }
            let subscription_id =
                non_empty_id(entries, "subscriptionId").ok_or_else(invalid_server)?;
            let update = entries
                .iter()
                .find(|(key, _)| key == "update")
                .map(|(_, value)| value.clone())
                .ok_or_else(invalid_server)?;
            Ok(ServerMessage::ServiceUpdate(ServiceEventEnvelope {
                subscription_id,
                update,
            }))
        }
        "attachment" => {
            if !exact_keys(entries, &["type", "attachment"]) {
                return Err(invalid_server());
            }
            let attachment = entries
                .iter()
                .find(|(key, _)| key == "attachment")
                .map(|(_, value)| value);
            let attachment = match attachment {
                Some(JsonValue::Null) => None,
                Some(value) => Some(parse_session_target(value).ok_or_else(invalid_server)?),
                None => return Err(invalid_server()),
            };
            Ok(ServerMessage::Attachment(AttachmentEnvelope { attachment }))
        }
        _ => Err(invalid_server()),
    }
}

/// Runtime re-validation on the encode path (upstream `encodeProtocolMessage`
/// re-parses before encoding). Typed values can still carry schema violations
/// (empty ids, non-canonical server ids, empty error codes).
pub fn validate_client_message(message: &ClientMessage) -> Result<(), ProtocolValidationError> {
    let check_target = |target: &RpcTarget| -> Result<(), ProtocolValidationError> {
        let (server_id, session_id, attachment_id) = match target {
            RpcTarget::Server(target) => (&target.server_id, None, None),
            RpcTarget::Session(target) => (
                &target.server_id,
                Some(&target.session_id),
                Some(&target.attachment_id),
            ),
        };
        if !is_server_id(server_id) {
            return Err(invalid_client());
        }
        if session_id.is_some_and(|id| id.is_empty())
            || attachment_id.is_some_and(|id| id.is_empty())
        {
            return Err(invalid_client());
        }
        Ok(())
    };
    match message {
        ClientMessage::Hello(_) => Ok(()),
        ClientMessage::Request(request) => {
            if request.id.is_empty() {
                return Err(invalid_client());
            }
            check_target(&request.target)
        }
        ClientMessage::Cancel(cancel) => {
            if cancel.id.is_empty() {
                return Err(invalid_client());
            }
            check_target(&cancel.target)
        }
    }
}

/// Runtime re-validation of a typed server message on the encode path.
pub fn validate_server_message(message: &ServerMessage) -> Result<(), ProtocolValidationError> {
    let check_error = |error: &ProtocolError| -> Result<(), ProtocolValidationError> {
        if error.code.is_empty() {
            return Err(invalid_server());
        }
        Ok(())
    };
    match message {
        ServerMessage::Hello(hello) => {
            if !is_server_id(&hello.server_id) {
                return Err(invalid_server());
            }
            Ok(())
        }
        ServerMessage::HelloError(error) => check_error(&error.error),
        ServerMessage::Response(response) => {
            if response.id.is_empty() {
                return Err(invalid_server());
            }
            match &response.outcome {
                ResponseOutcome::Success { .. } => Ok(()),
                ResponseOutcome::Failure { error } => check_error(error),
            }
        }
        ServerMessage::ServiceUpdate(event) => {
            if event.subscription_id.is_empty() {
                return Err(invalid_server());
            }
            Ok(())
        }
        ServerMessage::Attachment(attachment) => match &attachment.attachment {
            None => Ok(()),
            Some(target) => {
                if !is_server_id(&target.server_id)
                    || target.session_id.is_empty()
                    || target.attachment_id.is_empty()
                {
                    return Err(invalid_server());
                }
                Ok(())
            }
        },
    }
}

fn bounded_error_message(message: &str) -> String {
    let utf16_length: usize = message.chars().map(char::len_utf16).sum();
    if utf16_length <= 500 {
        return message.to_string();
    }
    let mut truncated = String::new();
    let mut units = 0;
    for character in message.chars() {
        if units + character.len_utf16() > 497 {
            break;
        }
        units += character.len_utf16();
        truncated.push(character);
    }
    truncated.push_str("...");
    truncated
}

fn encode_protocol_message(
    message_cbor: &crate::protocol::cbor::CborValue,
    kind: &str,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError> {
    let max_frame_length = options
        .and_then(|options| options.max_frame_length)
        .unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    let encoded = encode_cbor(
        message_cbor,
        CborOptions::new().with_max_byte_length(max_frame_length),
    );
    match encoded {
        Ok(payload) => encode_frame(&payload).map_err(|error| {
            ProtocolValidationError::new(format!(
                "Unable to encode {kind} protocol message: {}",
                bounded_error_message(error.message())
            ))
        }),
        Err(error) => Err(ProtocolValidationError::new(format!(
            "Unable to encode {kind} protocol message: {}",
            bounded_error_message(error.message())
        ))),
    }
}

/// Validates and encodes one complete length-prefixed client message.
pub fn encode_client_message(
    message: &ClientMessage,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError> {
    validate_client_message(message)?;
    encode_protocol_message(&message.to_cbor(), "client", options)
}

/// Validates and encodes one complete length-prefixed server message.
pub fn encode_server_message(
    message: &ServerMessage,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError> {
    validate_server_message(message)?;
    encode_protocol_message(&message.to_cbor(), "server", options)
}

struct ValidatedMessageDecoder<T> {
    failed: bool,
    frames: FrameDecoder,
    kind: &'static str,
    max_frame_length: u64,
    parse: fn(&JsonValue) -> Result<T, ProtocolValidationError>,
}

impl<T> ValidatedMessageDecoder<T> {
    fn new(
        kind: &'static str,
        parse: fn(&JsonValue) -> Result<T, ProtocolValidationError>,
        options: Option<FrameDecoderOptions>,
    ) -> Result<ValidatedMessageDecoder<T>, crate::protocol::cbor::RangeError> {
        Ok(ValidatedMessageDecoder {
            failed: false,
            frames: FrameDecoder::new(options.unwrap_or_default())?,
            kind,
            max_frame_length: options
                .and_then(|options| options.max_frame_length)
                .unwrap_or(DEFAULT_MAX_FRAME_LENGTH),
            parse,
        })
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<T>, ProtocolValidationError> {
        if self.failed {
            return Err(ProtocolValidationError::new(format!(
                "{} message decoder has failed",
                self.kind
            )));
        }
        match self.push_inner(chunk) {
            Ok(messages) => Ok(messages),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    fn push_inner(&mut self, chunk: &[u8]) -> Result<Vec<T>, ProtocolValidationError> {
        let frames = self.frames.push(chunk).map_err(|error: FrameError| {
            ProtocolValidationError::new(format!(
                "Invalid {} protocol frame: {}",
                self.kind,
                bounded_error_message(error.message())
            ))
        })?;
        let mut messages = Vec::new();
        for frame in frames {
            let decoded = decode_cbor(
                &frame,
                CborOptions::new().with_max_byte_length(self.max_frame_length),
            )
            .map_err(|error: CborFailure| {
                ProtocolValidationError::new(format!(
                    "Invalid {} protocol frame: {}",
                    self.kind,
                    bounded_error_message(error.message())
                ))
            })?;
            let value =
                cbor_value_to_json(&decoded).ok_or_else(|| invalid_message_error(self.kind))?;
            messages.push((self.parse)(&value)?);
        }
        Ok(messages)
    }

    fn end(&mut self) -> Result<(), ProtocolValidationError> {
        if self.failed {
            return Err(ProtocolValidationError::new(format!(
                "{} message decoder has failed",
                self.kind
            )));
        }
        self.frames.end().map_err(|error| {
            self.failed = true;
            ProtocolValidationError::new(format!(
                "Invalid {} protocol framing: {}",
                self.kind,
                bounded_error_message(error.message())
            ))
        })
    }
}

fn invalid_message_error(kind: &str) -> ProtocolValidationError {
    ProtocolValidationError::new(format!("Invalid {kind} protocol message"))
}

/// Incrementally decodes and validates framed client messages.
pub struct ClientMessageDecoder {
    decoder: ValidatedMessageDecoder<ClientMessage>,
}

impl ClientMessageDecoder {
    /// Returns `Err` for an out-of-range `maxFrameLength` (upstream
    /// `RangeError` from the frame decoder constructor).
    pub fn new(
        options: Option<FrameDecoderOptions>,
    ) -> Result<ClientMessageDecoder, crate::protocol::cbor::RangeError> {
        Ok(ClientMessageDecoder {
            decoder: ValidatedMessageDecoder::new("client", parse_client_message, options)?,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ClientMessage>, ProtocolValidationError> {
        self.decoder.push(chunk)
    }

    pub fn end(&mut self) -> Result<(), ProtocolValidationError> {
        self.decoder.end()
    }
}

/// Incrementally decodes and validates framed server messages.
pub struct ServerMessageDecoder {
    decoder: ValidatedMessageDecoder<ServerMessage>,
}

impl ServerMessageDecoder {
    /// Returns `Err` for an out-of-range `maxFrameLength` (upstream
    /// `RangeError` from the frame decoder constructor).
    pub fn new(
        options: Option<FrameDecoderOptions>,
    ) -> Result<ServerMessageDecoder, crate::protocol::cbor::RangeError> {
        Ok(ServerMessageDecoder {
            decoder: ValidatedMessageDecoder::new("server", parse_server_message, options)?,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ServerMessage>, ProtocolValidationError> {
        self.decoder.push(chunk)
    }

    pub fn end(&mut self) -> Result<(), ProtocolValidationError> {
        self.decoder.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::cbor::{json_value_to_cbor, CborValue};
    use crate::protocol::protocol::ServerTarget;

    const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn client_hello(version: u64) -> ClientMessage {
        ClientMessage::Hello(ClientHello { version })
    }

    fn server_hello() -> ServerMessage {
        ServerMessage::Hello(ServerHello {
            server_id: SERVER_ID.into(),
        })
    }

    fn hello_json(version: JsonValue) -> JsonValue {
        JsonValue::object(vec![
            ("type".into(), JsonValue::string("hello")),
            ("version".into(), version),
        ])
    }

    fn err_message<T>(result: Result<T, ProtocolValidationError>) -> String {
        match result {
            Err(error) => error.message().to_string(),
            Ok(_) => panic!("expected a validation error"),
        }
    }

    #[test]
    fn negotiates_protocol_version_eight() {
        assert!(is_supported_protocol_version(8.0));
        assert!(!is_supported_protocol_version(7.0));
        assert!(!is_supported_protocol_version(8.5));
    }

    #[test]
    fn accepts_integer_client_hello_versions_for_negotiation() {
        for version in [0u64, PROTOCOL_VERSION, PROTOCOL_VERSION + 1] {
            let message = hello_json(JsonValue::uint(version));
            assert_eq!(
                parse_client_message(&message).unwrap(),
                client_hello(version),
                "version {version}"
            );
        }
    }

    #[test]
    fn rejects_invalid_client_hellos() {
        // version: String(PROTOCOL_VERSION)
        assert!(parse_client_message(&hello_json(JsonValue::string("8"))).is_err());
        // version: PROTOCOL_VERSION + 0.5
        assert!(parse_client_message(&hello_json(JsonValue::Number(
            crate::protocol::json::Number::Float(8.5)
        )))
        .is_err());
        // extra: true
        assert!(parse_client_message(&JsonValue::object(vec![
            ("type".into(), JsonValue::string("hello")),
            ("version".into(), JsonValue::uint(8)),
            ("extra".into(), JsonValue::Bool(true)),
        ]))
        .is_err());
        // negative version fails the minimum.
        assert!(parse_client_message(&hello_json(JsonValue::Number(
            crate::protocol::json::Number::Int(-1)
        )))
        .is_err());
    }

    #[test]
    fn rejects_non_canonical_uuidv4_server_ids() {
        for server_id in [
            "",
            "server-1",
            "00000000-0000-7000-8000-000000000001",
            "00000000-0000-4000-7000-000000000001",
            "00000000-0000-4000-8000-00000000000A",
        ] {
            let message = JsonValue::object(vec![
                ("type".into(), JsonValue::string("request")),
                ("id".into(), JsonValue::string("request-1")),
                (
                    "target".into(),
                    JsonValue::object(vec![("serverId".into(), JsonValue::string(server_id))]),
                ),
                (
                    "call".into(),
                    JsonValue::object(vec![
                        ("serviceId".into(), JsonValue::string("pi.models")),
                        ("member".into(), JsonValue::string("list")),
                        ("args".into(), JsonValue::Array(vec![])),
                    ]),
                ),
            ]);
            assert_eq!(
                err_message(parse_client_message(&message)),
                "Invalid client protocol message",
                "server id {server_id}"
            );
        }
    }

    #[test]
    fn keeps_routed_request_and_event_payloads_opaque() {
        let request = ClientMessage::Request(RequestEnvelope {
            id: "request-1".into(),
            target: RpcTarget::Session(SessionTarget {
                server_id: SERVER_ID.into(),
                session_id: "session-1".into(),
                attachment_id: "attachment-1".into(),
            }),
            call: JsonValue::object(vec![
                ("serviceId".into(), JsonValue::string("application.custom")),
                (
                    "instance".into(),
                    JsonValue::object(vec![
                        ("key".into(), JsonValue::string("instance-1")),
                        ("generation".into(), JsonValue::uint(2)),
                    ]),
                ),
                ("member".into(), JsonValue::string("invoke")),
                (
                    "args".into(),
                    JsonValue::Array(vec![
                        JsonValue::object(vec![("arbitrary".into(), JsonValue::Bool(true))]),
                        JsonValue::Array(vec![JsonValue::string("opaque")]),
                    ]),
                ),
            ]),
        });
        let json = request_json(&request);
        assert_eq!(parse_client_message(&json).unwrap(), request);

        let arbitrary = JsonValue::object(vec![
            ("type".into(), JsonValue::string("request")),
            ("id".into(), JsonValue::string("request-1")),
            (
                "target".into(),
                JsonValue::object(vec![
                    ("serverId".into(), JsonValue::string(SERVER_ID)),
                    ("sessionId".into(), JsonValue::string("session-1")),
                    ("attachmentId".into(), JsonValue::string("attachment-1")),
                ]),
            ),
            (
                "call".into(),
                JsonValue::object(vec![(
                    "arbitrary".into(),
                    JsonValue::string("strict JSON whose service meaning belongs to Chord"),
                )]),
            ),
        ]);
        let parsed = parse_client_message(&arbitrary).unwrap();
        assert_eq!(
            parsed.to_cbor().get("call"),
            Some(&json_value_to_cbor(&JsonValue::object(vec![(
                "arbitrary".into(),
                JsonValue::string("strict JSON whose service meaning belongs to Chord")
            )])))
        );

        let update = ServerMessage::ServiceUpdate(ServiceEventEnvelope {
            subscription_id: "subscription-1".into(),
            update: JsonValue::object(vec![("applicationDefined".into(), JsonValue::Bool(true))]),
        });
        let parsed = parse_server_message(&update_json(&update)).unwrap();
        assert_eq!(parsed, update);
    }

    fn request_json(request: &ClientMessage) -> JsonValue {
        // Typed -> CBOR -> JSON is the wire-faithful JSON view.
        cbor_value_to_json(&request.to_cbor()).unwrap()
    }

    fn update_json(message: &ServerMessage) -> JsonValue {
        cbor_value_to_json(&message.to_cbor()).unwrap()
    }

    #[test]
    fn rejects_non_json_opaque_payloads() {
        // Byte arrays are the one non-JSON value that survives CBOR decoding
        // (D5: NaN, undefined properties, and cycles are unrepresentable in
        // the Rust value trees on every other path).
        let call_with_bytes = |bytes: Vec<u8>| {
            CborValue::Map(vec![
                ("type".into(), CborValue::Text("request".into())),
                ("id".into(), CborValue::Text("request-1".into())),
                (
                    "target".into(),
                    CborValue::Map(vec![("serverId".into(), CborValue::Text(SERVER_ID.into()))]),
                ),
                (
                    "call".into(),
                    CborValue::Map(vec![
                        (
                            "serviceId".into(),
                            CborValue::Text("application.custom".into()),
                        ),
                        ("member".into(), CborValue::Text("invoke".into())),
                        (
                            "args".into(),
                            CborValue::Array(vec![CborValue::Bytes(bytes)]),
                        ),
                    ]),
                ),
            ])
        };
        let wire =
            crate::protocol::cbor::encode_cbor(&call_with_bytes(vec![1]), CborOptions::new())
                .unwrap();
        let frame = encode_frame(&wire).unwrap();
        let mut decoder = ClientMessageDecoder::new(None).unwrap();
        assert_eq!(
            err_message(decoder.push(&frame)),
            "Invalid client protocol message"
        );

        let response_with_bytes = CborValue::Map(vec![
            ("type".into(), CborValue::Text("response".into())),
            ("id".into(), CborValue::Text("request-1".into())),
            ("ok".into(), CborValue::Bool(true)),
            ("result".into(), CborValue::Bytes(vec![1])),
        ]);
        let wire =
            crate::protocol::cbor::encode_cbor(&response_with_bytes, CborOptions::new()).unwrap();
        let frame = encode_frame(&wire).unwrap();
        let mut decoder = ServerMessageDecoder::new(None).unwrap();
        assert_eq!(
            err_message(decoder.push(&frame)),
            "Invalid server protocol message"
        );
        // ...and through the direct parse path.
        assert!(parse_client_message(&JsonValue::Array(vec![JsonValue::uint(1)])).is_err());
    }

    #[test]
    fn validates_request_cancellation_envelopes() {
        let cancel = ClientMessage::Cancel(CancelEnvelope {
            id: "request-1".into(),
            target: RpcTarget::Server(ServerTarget {
                server_id: SERVER_ID.into(),
            }),
        });
        assert_eq!(
            parse_client_message(&request_json(&cancel)).unwrap(),
            cancel
        );
        // id: ""
        let empty_id = JsonValue::object(vec![
            ("type".into(), JsonValue::string("cancel")),
            ("id".into(), JsonValue::string("")),
            (
                "target".into(),
                JsonValue::object(vec![("serverId".into(), JsonValue::string(SERVER_ID))]),
            ),
        ]);
        assert!(parse_client_message(&empty_id).is_err());
        // extra: true
        let extra = JsonValue::object(vec![
            ("type".into(), JsonValue::string("cancel")),
            ("id".into(), JsonValue::string("request-1")),
            (
                "target".into(),
                JsonValue::object(vec![("serverId".into(), JsonValue::string(SERVER_ID))]),
            ),
            ("extra".into(), JsonValue::Bool(true)),
        ]);
        assert!(parse_client_message(&extra).is_err());
    }

    #[test]
    fn validates_attachment_route_updates() {
        let attached = ServerMessage::Attachment(AttachmentEnvelope {
            attachment: Some(SessionTarget {
                server_id: SERVER_ID.into(),
                session_id: "session-1".into(),
                attachment_id: "attachment-1".into(),
            }),
        });
        let detached = ServerMessage::Attachment(AttachmentEnvelope { attachment: None });
        assert_eq!(
            parse_server_message(&update_json(&attached)).unwrap(),
            attached
        );
        assert_eq!(
            parse_server_message(&update_json(&detached)).unwrap(),
            detached
        );
        // Partial route {sessionId: "session-1"}.
        let partial = JsonValue::object(vec![
            ("type".into(), JsonValue::string("attachment")),
            (
                "attachment".into(),
                JsonValue::object(vec![("sessionId".into(), JsonValue::string("session-1"))]),
            ),
        ]);
        assert!(parse_server_message(&partial).is_err());
    }

    #[test]
    fn rejects_malformed_request_boundaries() {
        // empty request id
        let empty_id = JsonValue::object(vec![
            ("type".into(), JsonValue::string("request")),
            ("id".into(), JsonValue::string("")),
            (
                "target".into(),
                JsonValue::object(vec![("serverId".into(), JsonValue::string(SERVER_ID))]),
            ),
            (
                "call".into(),
                JsonValue::object(vec![
                    ("serviceId".into(), JsonValue::string("pi.models")),
                    ("member".into(), JsonValue::string("list")),
                    ("args".into(), JsonValue::Array(vec![])),
                ]),
            ),
        ]);
        assert_eq!(
            err_message(parse_client_message(&empty_id)),
            "Invalid client protocol message"
        );
        // extra envelope field
        let extra = JsonValue::object(vec![
            ("type".into(), JsonValue::string("request")),
            ("id".into(), JsonValue::string("request-1")),
            (
                "target".into(),
                JsonValue::object(vec![("serverId".into(), JsonValue::string(SERVER_ID))]),
            ),
            (
                "call".into(),
                JsonValue::object(vec![
                    ("serviceId".into(), JsonValue::string("pi.models")),
                    ("member".into(), JsonValue::string("list")),
                    ("args".into(), JsonValue::Array(vec![])),
                ]),
            ),
            ("extra".into(), JsonValue::Bool(true)),
        ]);
        assert!(parse_client_message(&extra).is_err());
    }

    #[test]
    fn accepts_a_successful_void_response_without_result_field() {
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("response")),
            ("id".into(), JsonValue::string("request-1")),
            ("ok".into(), JsonValue::Bool(true)),
        ]);
        assert_eq!(
            parse_server_message(&message).unwrap(),
            ServerMessage::Response(ResponseEnvelope {
                id: "request-1".into(),
                outcome: ResponseOutcome::Success { result: None },
            })
        );
    }

    #[test]
    fn rejects_malformed_server_boundaries() {
        // invalid server id
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("hello")),
            ("version".into(), JsonValue::uint(PROTOCOL_VERSION)),
            ("serverId".into(), JsonValue::string("server-1")),
        ]);
        assert!(parse_server_message(&message).is_err());
        // extra response field
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("response")),
            ("id".into(), JsonValue::string("request-1")),
            ("ok".into(), JsonValue::Bool(true)),
            ("result".into(), JsonValue::Array(vec![])),
            ("extra".into(), JsonValue::Bool(true)),
        ]);
        assert!(parse_server_message(&message).is_err());
        // empty error code
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("response")),
            ("id".into(), JsonValue::string("request-1")),
            ("ok".into(), JsonValue::Bool(false)),
            (
                "error".into(),
                JsonValue::object(vec![
                    ("code".into(), JsonValue::string("")),
                    ("message".into(), JsonValue::string("bad")),
                ]),
            ),
        ]);
        assert!(parse_server_message(&message).is_err());
        // error + ok:true is neither response variant.
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("response")),
            ("id".into(), JsonValue::string("request-1")),
            ("ok".into(), JsonValue::Bool(true)),
            (
                "error".into(),
                JsonValue::object(vec![
                    ("code".into(), JsonValue::string("x")),
                    ("message".into(), JsonValue::string("bad")),
                ]),
            ),
        ]);
        assert!(parse_server_message(&message).is_err());
    }

    #[test]
    fn accepts_the_opaque_error_codes() {
        for code in [
            "wrong_server",
            "cancelled",
            "service_not_found",
            "application_error",
        ] {
            let message = ServerMessage::Response(ResponseEnvelope {
                id: "request-1".into(),
                outcome: ResponseOutcome::Failure {
                    error: ProtocolError {
                        code: code.into(),
                        message: "safe".into(),
                    },
                },
            });
            assert_eq!(
                parse_server_message(&update_json(&message)).unwrap(),
                message
            );
        }
    }

    #[test]
    fn rejects_unknown_messages_and_fields() {
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("hello")),
            ("version".into(), JsonValue::uint(PROTOCOL_VERSION)),
            ("serverId".into(), JsonValue::string(SERVER_ID)),
            ("snapshot".into(), JsonValue::object(vec![])),
        ]);
        assert!(parse_server_message(&message).is_err());
        let message = JsonValue::object(vec![
            ("type".into(), JsonValue::string("unknown")),
            ("event".into(), JsonValue::object(vec![])),
        ]);
        assert!(parse_server_message(&message).is_err());
    }

    #[test]
    fn does_not_parse_json_strings_as_messages() {
        let client = JsonValue::string("{\"type\":\"hello\",\"version\":8}");
        assert!(parse_client_message(&client).is_err());
        let server = JsonValue::string("{\"type\":\"hello\",\"version\":8,\"serverId\":\"\"}");
        assert!(parse_server_message(&server).is_err());
    }

    #[test]
    fn encodes_complete_client_and_server_frames() {
        let client_frame = encode_client_message(&client_hello(PROTOCOL_VERSION), None).unwrap();
        let payload = FrameDecoder::new(FrameDecoderOptions::default())
            .unwrap()
            .push(&client_frame)
            .unwrap()
            .remove(0);
        assert_eq!(
            parse_client_message(
                &cbor_value_to_json(&decode_cbor(&payload, CborOptions::new()).unwrap()).unwrap()
            )
            .unwrap(),
            client_hello(PROTOCOL_VERSION)
        );
        // Oracle frame hex for the hello exchange.
        assert_eq!(
            hex(&client_frame),
            "00000015a264747970656568656c6c6f6776657273696f6e08"
        );
        let server_frame = encode_server_message(&server_hello(), None).unwrap();
        assert_eq!(hex(&server_frame), "00000044a364747970656568656c6c6f6776657273696f6e08687365727665724964782430303030303030302d303030302d343030302d383030302d303030303030303030303031");
    }

    #[test]
    fn enforces_outbound_frame_limits() {
        let error = encode_client_message(
            &client_hello(PROTOCOL_VERSION),
            Some(FrameDecoderOptions::default().with_max_frame_length(8)),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "Unable to encode client protocol message: CBOR byte length exceeds configured limit of 8"
        );
        let error = encode_server_message(
            &server_hello(),
            Some(FrameDecoderOptions::default().with_max_frame_length(8)),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "Unable to encode server protocol message: CBOR byte length exceeds configured limit of 8"
        );
    }

    #[test]
    fn incrementally_decodes_fragmented_and_coalesced_client_messages() {
        let request = ClientMessage::Request(RequestEnvelope {
            id: "request-1".into(),
            target: RpcTarget::Server(ServerTarget {
                server_id: SERVER_ID.into(),
            }),
            call: JsonValue::object(vec![
                (
                    "serviceId".into(),
                    JsonValue::string("pi.session-directory"),
                ),
                ("member".into(), JsonValue::string("list")),
                ("args".into(), JsonValue::Array(vec![])),
            ]),
        });
        let first = encode_client_message(&client_hello(PROTOCOL_VERSION), None).unwrap();
        let second = encode_client_message(&request, None).unwrap();
        let mut wire = first.clone();
        wire.extend_from_slice(&second);

        for split in 0..=wire.len() {
            let mut decoder = ClientMessageDecoder::new(None).unwrap();
            let mut messages = decoder.push(&wire[..split]).unwrap();
            messages.extend(decoder.push(&wire[split..]).unwrap());
            decoder.end().unwrap();
            assert_eq!(
                messages,
                [client_hello(PROTOCOL_VERSION), request.clone()],
                "split {split}"
            );
        }
    }

    #[test]
    fn incrementally_decodes_fragmented_and_coalesced_server_messages() {
        let response = ServerMessage::Response(ResponseEnvelope {
            id: "request-1".into(),
            outcome: ResponseOutcome::Success {
                result: Some(JsonValue::Array(vec![])),
            },
        });
        let first = encode_server_message(&server_hello(), None).unwrap();
        let second = encode_server_message(&response, None).unwrap();
        let split = first.len() + second.len() / 2;
        let mut wire = first.clone();
        wire.extend_from_slice(&second);

        let mut decoder = ServerMessageDecoder::new(None).unwrap();
        assert_eq!(decoder.push(&wire[..split]).unwrap(), [server_hello()]);
        assert_eq!(decoder.push(&wire[split..]).unwrap(), [response]);
        decoder.end().unwrap();
    }

    #[test]
    fn rejects_invalid_framed_input_and_stays_failed() {
        let cases: Vec<Vec<u8>> = vec![
            // empty CBOR payload
            encode_frame(&[]).unwrap(),
            // malformed CBOR
            encode_frame(&[0xff]).unwrap(),
            // schema-invalid CBOR
            encode_frame(
                &crate::protocol::cbor::encode_cbor(
                    &CborValue::Map(vec![
                        ("type".into(), CborValue::Text("hello".into())),
                        ("version".into(), CborValue::Uint(1)),
                        ("extra".into(), CborValue::Bool(true)),
                    ]),
                    CborOptions::new(),
                )
                .unwrap(),
            )
            .unwrap(),
        ];
        for wire in cases {
            let mut decoder = ClientMessageDecoder::new(None).unwrap();
            assert!(decoder.push(&wire).is_err(), "wire {}", hex(&wire));
            assert_eq!(
                err_message(
                    decoder.push(
                        &encode_client_message(&client_hello(PROTOCOL_VERSION), None).unwrap()
                    )
                ),
                "client message decoder has failed"
            );
        }
    }

    #[test]
    fn rejects_truncated_and_oversized_framing() {
        let mut truncated = ServerMessageDecoder::new(None).unwrap();
        assert_eq!(
            truncated.push(&[0, 0, 0, 2, 1]).unwrap(),
            Vec::<ServerMessage>::new()
        );
        assert!(truncated.end().is_err());

        let mut oversized = ClientMessageDecoder::new(Some(
            FrameDecoderOptions::default().with_max_frame_length(3),
        ))
        .unwrap();
        assert_eq!(
            err_message(oversized.push(&[0, 0, 0, 4])),
            "Invalid client protocol frame: Frame length 4 exceeds configured limit of 3"
        );
        assert_eq!(
            err_message(oversized.push(&[1])),
            "client message decoder has failed"
        );
    }

    #[test]
    fn frames_all_oracle_messages_byte_for_byte() {
        let session_target = SessionTarget {
            server_id: SERVER_ID.into(),
            session_id: "session-1".into(),
            attachment_id: "attachment-1".into(),
        };
        let request_session = ClientMessage::Request(RequestEnvelope {
            id: "request-2".into(),
            target: RpcTarget::Session(session_target.clone()),
            call: JsonValue::object(vec![
                ("serviceId".into(), JsonValue::string("application.custom")),
                (
                    "instance".into(),
                    JsonValue::object(vec![
                        ("key".into(), JsonValue::string("instance-1")),
                        ("generation".into(), JsonValue::uint(2)),
                    ]),
                ),
                ("member".into(), JsonValue::string("invoke")),
                (
                    "args".into(),
                    JsonValue::Array(vec![
                        JsonValue::object(vec![("arbitrary".into(), JsonValue::Bool(true))]),
                        JsonValue::Array(vec![JsonValue::string("opaque")]),
                    ]),
                ),
            ]),
        });
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                encode_client_message(&client_hello(0), None).unwrap(),
                "00000015a264747970656568656c6c6f6776657273696f6e00",
            ),
            (
                encode_client_message(&client_hello(9), None).unwrap(),
                "00000015a264747970656568656c6c6f6776657273696f6e09",
            ),
            (
                encode_client_message(&request_session, None).unwrap(),
                "000000f0a46474797065677265717565737462696469726571756573742d3266746172676574a3687365727665724964782430303030303030302d303030302d343030302d383030302d3030303030303030303030316973657373696f6e49646973657373696f6e2d316c6174746163686d656e7449646c6174746163686d656e742d316463616c6ca469736572766963654964726170706c69636174696f6e2e637573746f6d68696e7374616e6365a2636b65796a696e7374616e63652d316a67656e65726174696f6e02666d656d62657266696e766f6b65646172677382a169617262697472617279f581666f7061717565",
            ),
            (
                encode_client_message(
                    &ClientMessage::Cancel(CancelEnvelope {
                        id: "request-1".into(),
                        target: RpcTarget::Server(ServerTarget {
                            server_id: SERVER_ID.into(),
                        }),
                    }),
                    None,
                )
                .unwrap(),
                "00000051a364747970656663616e63656c62696469726571756573742d3166746172676574a1687365727665724964782430303030303030302d303030302d343030302d383030302d303030303030303030303031",
            ),
            (
                encode_server_message(
                    &ServerMessage::HelloError(ServerHelloError {
                        error: ProtocolError {
                            code: "wrong_server".into(),
                            message: "safe".into(),
                        },
                    }),
                    None,
                )
                .unwrap(),
                "00000038a264747970656b68656c6c6f5f6572726f72656572726f72a264636f64656c77726f6e675f736572766572676d6573736167656473616665",
            ),
            (
                encode_server_message(
                    &ServerMessage::Response(ResponseEnvelope {
                        id: "request-1".into(),
                        outcome: ResponseOutcome::Success { result: None },
                    }),
                    None,
                )
                .unwrap(),
                "00000020a3647479706568726573706f6e736562696469726571756573742d31626f6bf5",
            ),
            (
                encode_server_message(
                    &ServerMessage::Response(ResponseEnvelope {
                        id: "request-1".into(),
                        outcome: ResponseOutcome::Success {
                            result: Some(JsonValue::Array(vec![])),
                        },
                    }),
                    None,
                )
                .unwrap(),
                "00000028a4647479706568726573706f6e736562696469726571756573742d31626f6bf566726573756c7480",
            ),
            (
                encode_server_message(
                    &ServerMessage::Response(ResponseEnvelope {
                        id: "request-1".into(),
                        outcome: ResponseOutcome::Failure {
                            error: ProtocolError {
                                code: "service_not_found".into(),
                                message: "safe".into(),
                            },
                        },
                    }),
                    None,
                )
                .unwrap(),
                "0000004ba4647479706568726573706f6e736562696469726571756573742d31626f6bf4656572726f72a264636f646571736572766963655f6e6f745f666f756e64676d6573736167656473616665",
            ),
            (
                encode_server_message(
                    &ServerMessage::ServiceUpdate(ServiceEventEnvelope {
                        subscription_id: "subscription-1".into(),
                        update: JsonValue::object(vec![(
                            "applicationDefined".into(),
                            JsonValue::Bool(true),
                        )]),
                    }),
                    None,
                )
                .unwrap(),
                "0000004fa364747970656e736572766963655f7570646174656e737562736372697074696f6e49646e737562736372697074696f6e2d3166757064617465a1726170706c69636174696f6e446566696e6564f5",
            ),
            (
                encode_server_message(
                    &ServerMessage::Attachment(AttachmentEnvelope {
                        attachment: Some(session_target),
                    }),
                    None,
                )
                .unwrap(),
                "0000007aa264747970656a6174746163686d656e746a6174746163686d656e74a3687365727665724964782430303030303030302d303030302d343030302d383030302d3030303030303030303030316973657373696f6e49646973657373696f6e2d316c6174746163686d656e7449646c6174746163686d656e742d31",
            ),
            (
                encode_server_message(
                    &ServerMessage::Attachment(AttachmentEnvelope { attachment: None }),
                    None,
                )
                .unwrap(),
                "0000001da264747970656a6174746163686d656e746a6174746163686d656e74f6",
            ),
        ];
        for (frame, oracle) in cases {
            assert_eq!(hex(&frame), oracle);
        }
    }

    #[test]
    fn bounded_error_message_truncates_like_upstream() {
        assert_eq!(bounded_error_message("short"), "short");
        let long = "x".repeat(500);
        assert_eq!(bounded_error_message(&long), long);
        let long = "y".repeat(501);
        assert_eq!(
            bounded_error_message(&long),
            format!("{}...", "y".repeat(497))
        );
    }

    #[test]
    fn validate_rejects_schema_violations_on_the_encode_path() {
        let bad_request = ClientMessage::Request(RequestEnvelope {
            id: String::new(),
            target: RpcTarget::Server(ServerTarget {
                server_id: SERVER_ID.into(),
            }),
            call: JsonValue::Null,
        });
        assert_eq!(
            err_message(encode_client_message(&bad_request, None)),
            "Invalid client protocol message"
        );
        let bad_hello = ServerMessage::Hello(ServerHello {
            server_id: "server-1".into(),
        });
        assert_eq!(
            err_message(encode_server_message(&bad_hello, None)),
            "Invalid server protocol message"
        );
    }
}
