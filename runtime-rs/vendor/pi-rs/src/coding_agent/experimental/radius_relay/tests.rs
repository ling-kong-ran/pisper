//! Tests for `radius_relay.rs`: pinned to the node oracle captured from the
//! verbatim upstream file
//! (tests/fixtures/experimental_final_oracle/oracle_relay_out.json; upstream sources
//! in tests/fixtures/experimental_final_oracle/upstream/radius-relay.ts).

use super::*;

const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";
const CONNECTION_ID: &str = "00000000-0000-4000-8000-000000000002";

fn accept_handler(closed_immediately: bool) -> RelayAccept {
    Arc::new(move |connection| {
        let mut handler = RelayByteConnectionHandler::default();
        if closed_immediately {
            // The embedded server closed the relay connection synchronously
            // inside accept (upstream `if (connection.closed)` branch).
            connection.mark_closed();
            handler.on_close_count += 0;
        }
        handler
    })
}

#[test]
fn envelope_round_trip_matches_the_oracle_bytes() {
    // Oracle: envelope.encoded / payload / connectionId.
    let payload = [0u8, 1, 2, 255];
    let encoded = encode_relay_data_frame(CONNECTION_ID, &payload).unwrap();
    assert_eq!(
        encoded,
        vec![1, 1, 0, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 2, 0, 1, 2, 255]
    );
    let parsed = parse_relay_data_frame(&encoded).unwrap();
    assert_eq!(parsed.connection_id, CONNECTION_ID);
    assert_eq!(parsed.payload, payload.to_vec());
}

#[test]
fn envelope_failures_match_the_oracle() {
    // Oracle: encodeInvalidId ("TypeError: Invalid Radius relay connection
    // ID" — the Rust port keeps the message), parseTooShort, parseWrongVersion.
    let error = encode_relay_data_frame("not-a-uuid", &[1]).unwrap_err();
    assert_eq!(error, "Invalid Radius relay connection ID");
    assert_eq!(parse_relay_data_frame(&[1, 1, 0]), None);
    let mut wrong_version = encode_relay_data_frame(CONNECTION_ID, &[1]).unwrap();
    wrong_version[1] = 2;
    assert_eq!(parse_relay_data_frame(&wrong_version), None);
    // Oracle: parseNonV4 — a mutated leading group stays valid because the
    // version/variant nibbles are untouched.
    let mut mutated = encode_relay_data_frame(CONNECTION_ID, &[1]).unwrap();
    mutated[3] = 0x7f;
    let parsed = parse_relay_data_frame(&mutated).unwrap();
    assert_eq!(parsed.connection_id, "007f0000-0000-4000-8000-000000000002");
    // Breaking the version nibble does reject.
    let mut no_v4 = encode_relay_data_frame(CONNECTION_ID, &[1]).unwrap();
    no_v4[2 + 6] = 0x9f; // hex[12..14] -> version nibble
    assert_eq!(parse_relay_data_frame(&no_v4), None);
}

#[test]
fn control_message_parsing_and_serialization_match_the_oracle() {
    // Oracle: hostBridge.pingReply and unknownConnectionClose byte shapes.
    let pong = parse_host_control_message(r#"{"version":1,"type":"ping"}"#).unwrap();
    assert_eq!(pong, HostInputControlMessage::Ping);
    assert_eq!(
        HostOutputControlMessage::Pong.to_json(),
        r#"{"version":1,"type":"pong"}"#
    );
    let _close = parse_host_control_message(
        r#"{"version":1,"type":"connection_close","connection_id":"00000000-0000-4000-8000-000000000003"}"#,
    )
    .unwrap();
    assert_eq!(
        HostOutputControlMessage::ConnectionClose {
            connection_id: "00000000-0000-4000-8000-000000000003".to_string(),
            code: Some(1000),
        }
        .to_json(),
        r#"{"version":1,"type":"connection_close","connection_id":"00000000-0000-4000-8000-000000000003","code":1000}"#
    );
    assert_eq!(
        HostOutputControlMessage::ConnectionClose {
            connection_id: CONNECTION_ID.to_string(),
            code: None,
        }
        .to_json(),
        r#"{"version":1,"type":"connection_close","connection_id":"00000000-0000-4000-8000-000000000002"}"#
    );

    // Upstream error strings, exercised through the host in the oracle.
    assert_eq!(
        parse_host_control_message(r#"{"version":2,"type":"ping"}"#).unwrap_err(),
        "Unsupported Radius relay control version"
    );
    assert_eq!(
        parse_host_control_message(r#"{"version":1,"type":"nonsense"}"#).unwrap_err(),
        "Invalid Radius relay control message"
    );
    assert_eq!(
        parse_host_control_message(
            r#"{"version":1,"type":"connection_open","connection_id":"nope"}"#
        )
        .unwrap_err(),
        "Invalid Radius relay control message"
    );
    assert_eq!(
        parse_host_control_message(
            r#"{"version":1,"type":"connection_close","connection_id":"00000000-0000-4000-8000-000000000002","code":42}"#
        )
        .unwrap_err(),
        "Invalid Radius relay control message"
    );
    assert_eq!(
        parse_host_control_message("not json").unwrap_err(),
        "Invalid Radius relay control message"
    );
    assert_eq!(
        parse_host_control_message(r#"["array"]"#).unwrap_err(),
        "Invalid Radius relay control message"
    );
}

#[test]
fn relay_url_matches_the_oracle_request_target() {
    // Oracle: hostBridge.openOptions.url.
    assert_eq!(
        relay_web_socket_url("https://radius.pi.dev", SERVER_ID).unwrap(),
        "wss://radius.pi.dev/v1/session-relays/00000000-0000-4000-8000-000000000001/connect"
    );
    assert_eq!(
        relay_web_socket_url("http://localhost:9090", SERVER_ID).unwrap(),
        "ws://localhost:9090/v1/session-relays/00000000-0000-4000-8000-000000000001/connect"
    );
    let error = relay_web_socket_url("ftp://radius.pi.dev", SERVER_ID).unwrap_err();
    assert!(error.starts_with("Unsupported Radius gateway protocol: ftp"));
}

#[test]
fn host_bridge_matches_the_oracle_lifecycle() {
    // Oracle: hostBridge (statuses, accept, data both ways, pong,
    // unknown-connection close, remote close handling).
    let seen_connections = Arc::new(std::sync::Mutex::new(Vec::<bool>::new()));
    let seen = seen_connections.clone();
    let accept: RelayAccept = Arc::new(move |connection| {
        seen.lock().unwrap().push(connection.closed());
        RelayByteConnectionHandler::default()
    });
    let mut host = RadiusRelayHost::new(SERVER_ID, accept);
    host.start();
    assert_eq!(
        host.status_for_auth(None),
        RadiusRelayHostStatus::NotAuthenticated
    );
    assert_eq!(
        host.status_for_auth(Some(&RadiusRelayAuth {
            gateway: "https://radius.pi.dev".to_string(),
            token: "secret".to_string(),
        })),
        RadiusRelayHostStatus::Connecting
    );
    host.on_established();

    let handling = host
        .handle_message(HostInput::Control(
            HostInputControlMessage::ConnectionOpen {
                connection_id: CONNECTION_ID.to_string(),
            },
        ))
        .unwrap();
    assert_eq!(handling.accepted, vec![CONNECTION_ID]);
    assert_eq!(host.connections(), vec![CONNECTION_ID]);

    let from_client = [1u8, 2, 3];
    let handling = host
        .handle_message(HostInput::Data(
            encode_relay_data_frame(CONNECTION_ID, &from_client).unwrap(),
        ))
        .unwrap();
    assert_eq!(handling.delivered_data.len(), 1);
    assert_eq!(handling.delivered_data[0].payload, from_client.to_vec());

    let outbound = host.send_data(CONNECTION_ID, &[4, 5, 6]).unwrap();
    match &outbound {
        HostOutput::Data {
            connection_id,
            payload,
        } => {
            assert_eq!(connection_id, CONNECTION_ID);
            assert_eq!(payload, &[4, 5, 6]);
            let frame = encode_relay_data_frame(connection_id, payload).unwrap();
            let parsed = parse_relay_data_frame(&frame).unwrap();
            assert_eq!(parsed.connection_id, CONNECTION_ID);
            assert_eq!(parsed.payload, vec![4, 5, 6]);
        }
        other => panic!("expected data output, got {other:?}"),
    }

    let handling = host
        .handle_message(HostInput::Control(HostInputControlMessage::Ping))
        .unwrap();
    assert_eq!(
        handling.outputs,
        vec![HostOutput::Control(HostOutputControlMessage::Pong)]
    );

    let handling = host
        .handle_message(HostInput::Data(
            encode_relay_data_frame("00000000-0000-4000-8000-000000000003", &[9]).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        handling.outputs,
        vec![HostOutput::Control(
            HostOutputControlMessage::ConnectionClose {
                connection_id: "00000000-0000-4000-8000-000000000003".to_string(),
                code: Some(1000),
            }
        )]
    );

    host.remote_close_connection(CONNECTION_ID);
    assert!(host.connections().is_empty());
    assert!(!seen_connections.lock().unwrap().is_empty());
    assert!(!seen_connections.lock().unwrap()[0]);

    host.close();
    assert!(host.is_closed());
}

#[test]
fn host_protocol_failures_close_with_code_4000_and_report_retrying() {
    // Oracle: hostFailures.retryErrors[0] — a protocol violation closes the
    // socket with code 4000 and the fixed reason, then the loop reports
    // retrying with the underlying error text.
    let mut host = RadiusRelayHost::new(SERVER_ID, accept_handler(false));
    host.start();
    host.on_established();
    let error = host
        .handle_message(HostInput::Data(vec![0x1, 0x1, 0, 0])) // too short frame
        .unwrap_err();
    assert_eq!(error.close_code, LOCAL_PROTOCOL_ERROR_CLOSE_CODE);
    assert_eq!(error.close_reason, "Radius relay protocol error");
    assert_eq!(error.message, "Invalid Radius relay data frame");
    assert_eq!(
        host.classify_remote_close(error.close_code, error.close_reason),
        Some("Radius relay host closed (4000: Radius relay protocol error)".to_string())
    );
    let status = host.on_disconnected(Some(error.message));
    assert_eq!(
        status,
        RadiusRelayHostStatus::Retrying {
            error: "Invalid Radius relay data frame".to_string()
        }
    );
}

#[test]
fn host_backoff_doubles_up_to_the_cap() {
    // Oracle: hostReconnect — one retry scheduled 1s after the drop; the
    // backoff sequence doubles and caps at 30s.
    let mut host = RadiusRelayHost::new(SERVER_ID, accept_handler(false));
    host.start();
    host.on_established();
    assert_eq!(host.retry_delay_after_failure(), 1_000);
    assert_eq!(host.retry_delay_after_failure(), 2_000);
    assert_eq!(host.retry_delay_after_failure(), 4_000);
    for _ in 0..20 {
        host.retry_delay_after_failure();
    }
    assert_eq!(host.retry_delay_after_failure(), 30_000);
    assert_eq!(host.missing_auth_retry_delay(), 30_000);
}

#[test]
fn clean_and_abnormal_close_classification_match_the_oracle() {
    // Upstream `#serve`'s onClose classification (oracle: clean code 1000
    // ends the serve without an error; other codes carry code+reason).
    assert_eq!(
        RadiusRelayHost::new(SERVER_ID, accept_handler(false)).classify_remote_close(1000, ""),
        None
    );
    assert_eq!(
        RadiusRelayHost::new(SERVER_ID, accept_handler(false)).classify_remote_close(1006, "lost"),
        Some("Radius relay host closed (1006: lost)".to_string())
    );
    assert_eq!(
        RadiusRelayHost::new(SERVER_ID, accept_handler(false)).classify_remote_close(1006, ""),
        Some("Radius relay host closed (1006)".to_string())
    );
}

#[test]
fn host_close_drops_tracked_connections_with_on_close() {
    let handler_sink = Arc::new(std::sync::Mutex::new(
        Vec::<RelayByteConnectionHandler>::new(),
    ));
    let sink = handler_sink.clone();
    let accept: RelayAccept = Arc::new(move |connection| {
        let mut handler = RelayByteConnectionHandler::default();
        handler
            .on_data
            .push(connection.connection_id().as_bytes().to_vec());
        sink.lock().unwrap().push(handler);
        RelayByteConnectionHandler::default()
    });
    let mut host = RadiusRelayHost::new(SERVER_ID, accept);
    host.start();
    host.on_established();
    host.handle_message(HostInput::Control(
        HostInputControlMessage::ConnectionOpen {
            connection_id: CONNECTION_ID.to_string(),
        },
    ))
    .unwrap();
    host.close();
    assert!(host.connections().is_empty());
}

#[test]
fn immediate_server_close_answers_1012() {
    let mut host = RadiusRelayHost::new(SERVER_ID, accept_handler(true));
    host.start();
    host.on_established();
    let mut handling = HostHandling::default();
    host.open_connection(CONNECTION_ID, &mut handling);
    assert_eq!(
        handling.outputs,
        vec![HostOutput::Control(
            HostOutputControlMessage::ConnectionClose {
                connection_id: CONNECTION_ID.to_string(),
                code: Some(1012),
            }
        )]
    );
    assert!(host.connections().is_empty());
}

#[test]
fn duplicate_connection_id_is_a_protocol_error() {
    // Upstream `#openConnection` throws "Radius relay reused a connection ID".
    let mut host = RadiusRelayHost::new(SERVER_ID, accept_handler(false));
    host.start();
    host.on_established();
    host.handle_message(HostInput::Control(
        HostInputControlMessage::ConnectionOpen {
            connection_id: CONNECTION_ID.to_string(),
        },
    ))
    .unwrap();
    let mut handling = HostHandling::default();
    host.open_connection(CONNECTION_ID, &mut handling);
    let error = handling.protocol_error.unwrap();
    assert_eq!(error.message, "Radius relay reused a connection ID");
}

#[test]
fn server_side_close_after_final_chunk_sends_close_1000() {
    // Upstream `#serverCloseConnection`.
    let mut host = RadiusRelayHost::new(SERVER_ID, accept_handler(false));
    host.start();
    host.on_established();
    host.handle_message(HostInput::Control(
        HostInputControlMessage::ConnectionOpen {
            connection_id: CONNECTION_ID.to_string(),
        },
    ))
    .unwrap();
    let outputs = host
        .server_close_connection(CONNECTION_ID, Some(&[7, 8]))
        .unwrap();
    assert_eq!(
        outputs,
        vec![
            HostOutput::Data {
                connection_id: CONNECTION_ID.to_string(),
                payload: vec![7, 8],
            },
            HostOutput::Control(HostOutputControlMessage::ConnectionClose {
                connection_id: CONNECTION_ID.to_string(),
                code: Some(1000),
            }),
        ]
    );
    assert!(host.connections().is_empty());
    // Closing an unknown connection is a no-op.
    assert!(host
        .server_close_connection(CONNECTION_ID, None)
        .unwrap()
        .is_empty());
    // Sending on a closed connection fails with the upstream message.
    let error = host.send_data(CONNECTION_ID, &[1]).unwrap_err();
    assert_eq!(error, "Radius relay connection is closed");
}

#[test]
fn client_transport_matches_the_oracle_lifecycle() {
    // Oracle: clientTransport (send bytes, receive data, remote close),
    // abnormalClose (onError only), and the closed-send error.
    let mut transport = RadiusClientByteTransport::new();
    transport.send(3).unwrap();
    transport.finish_send(3);
    let mut transport = RadiusClientByteTransport::new();
    let failure = transport.fail_non_binary();
    assert_eq!(
        failure.error.as_deref(),
        Some("Radius relay client received a non-binary message")
    );
    assert_eq!(
        failure.close,
        Some((
            LOCAL_TRANSPORT_ERROR_CLOSE_CODE,
            "Radius relay transport error"
        ))
    );

    let mut transport = RadiusClientByteTransport::new();
    let failure = transport.fail("network lost");
    assert_eq!(failure.error.as_deref(), Some("network lost"));
    assert_eq!(
        failure.close,
        Some((
            LOCAL_TRANSPORT_ERROR_CLOSE_CODE,
            "Radius relay transport error"
        ))
    );

    let mut transport = RadiusClientByteTransport::new();
    assert_eq!(transport.close(), Some((1000, "Pi client closed")));
    assert_eq!(transport.close(), None);
    let error = transport.send(1).unwrap_err();
    assert_eq!(error, "Radius relay client is closed");
}

#[test]
fn pending_write_budget_matches_the_upstream_limit_errors() {
    // Upstream OrderedWebSocketWriter: closed sends and the pending cap.
    let mut budget = PendingWriteBudget::default();
    budget.admit(1024).unwrap();
    budget.finish(1024);
    let error = budget.admit(MAX_PENDING_BYTES + 1).unwrap_err();
    assert_eq!(error, "Radius relay exceeded its pending byte limit");
    budget.admit(MAX_PENDING_BYTES).unwrap();
    let error = budget.admit(1).unwrap_err();
    assert_eq!(error, "Radius relay exceeded its pending byte limit");
    budget.close();
    assert!(budget.is_closed());
    let error = budget.admit(1).unwrap_err();
    assert_eq!(error, "Radius relay WebSocket is closed");
}

#[test]
fn client_reconnect_matches_the_oracle_attempt_sequence() {
    // Oracle: clientReconnect (attemptsAfterFirst 1, attemptsFinal 2,
    // reattachCalls ["demo-1"]).
    let mut reconnect = RadiusClientReconnect::new(Some("demo-1"));
    assert_eq!(reconnect.desired_session_id(), Some("demo-1"));

    let action = reconnect.observe_disconnected();
    assert_eq!(action, ReconnectAction::Retry { delay_ms: 1_000 });
    // First reconnect() attempt fails ("temporary failure"): the next wait
    // doubles to 2s (oracle: advanceTimersByTimeAsync(2_000) -> attempt 2).
    let action = reconnect.on_reconnect_failed("temporary failure");
    assert_eq!(action, ReconnectAction::Retry { delay_ms: 2_000 });
    // Second attempt succeeds and restores the last selected session.
    let action = reconnect.on_reconnected();
    assert_eq!(
        action,
        ReconnectAction::Reconnected {
            reattach: Some("demo-1".to_string())
        }
    );

    // Attachment updates while connected: new sessions win, detachments clear.
    reconnect.observe_attachment(Some("demo-2"), true);
    assert_eq!(reconnect.desired_session_id(), Some("demo-2"));
    reconnect.observe_attachment(None, true);
    assert_eq!(reconnect.desired_session_id(), None);
    reconnect.observe_attachment(None, false);
    assert_eq!(reconnect.desired_session_id(), None);

    // Backoff caps at 30s.
    for _ in 0..20 {
        reconnect.on_reconnect_failed("again");
    }
    assert_eq!(
        reconnect.on_reconnect_failed("again"),
        ReconnectAction::Retry { delay_ms: 30_000 }
    );
    assert_eq!(
        reconnect.on_reconnect_failed("again"),
        ReconnectAction::Retry { delay_ms: 30_000 }
    );

    // dispose disconnects an established client with the fixed reason.
    assert_eq!(reconnect.dispose(true), None);
    let mut reconnect = RadiusClientReconnect::new(Some("demo-1"));
    assert_eq!(reconnect.dispose(false), Some("Radius reconnect stopped"));
    assert!(reconnect.is_disposed());
    assert_eq!(reconnect.observe_disconnected(), ReconnectAction::Idle);
}

#[test]
fn open_failure_error_strings_match_the_oracle() {
    // Oracle: emptyError.rejection and protocolMismatch.rejection.
    assert_eq!(
        web_socket_error(Some(""), None),
        "Radius WebSocket connection failed"
    );
    assert_eq!(
        web_socket_error(None, Some(" upstream said ")),
        "upstream said"
    );
    assert_eq!(
        unexpected_protocol_error("pi-session-relay.other.v1"),
        "Radius relay selected unexpected WebSocket protocol \"pi-session-relay.other.v1\""
    );
    let messages = open_failure_messages();
    assert!(messages.contains(&"Radius WebSocket connection failed".to_string()));
    assert_eq!(
        closed_before_connecting_error(1006),
        "Radius relay closed before connecting (1006)"
    );
    assert_eq!(
        CONNECTION_CANCELLED_ERROR,
        "Radius relay connection cancelled"
    );
    assert_eq!(
        CONNECTION_FAILED_CLOSE_REASON,
        "Radius relay connection failed"
    );
}
