//! Tests for `coordinator.rs`: validation messages and reply-frame shapes
//! pinned to upstream `coordinator.ts` (`registerServer`, `registerPeer`,
//! `handleRoutedMessage`) and node-oracle byte output
//! (tests/fixtures/experimental_oracle/oracle_output.json).

use super::*;

fn register_server() -> RegisterServerFrame {
    RegisterServerFrame {
        message_type: "register_server".to_string(),
        protocol: COORDINATOR_PROTOCOL_VERSION,
        server_connection_id: "srv-1".to_string(),
        endpoint: "127.0.0.1:9000".to_string(),
    }
}

fn register_peer() -> RegisterPeerFrame {
    RegisterPeerFrame {
        message_type: "register_peer".to_string(),
        protocol: COORDINATOR_PROTOCOL_VERSION,
        peer_id: "worker-abc".to_string(),
        server_connection_id: None,
    }
}

#[test]
fn accepts_valid_registration_frames() {
    assert_eq!(
        validate_register_server(&register_server()).unwrap(),
        ("srv-1".to_string(), "127.0.0.1:9000".to_string())
    );
    assert_eq!(
        validate_register_peer(&register_peer(), &[]).unwrap(),
        "worker-abc"
    );
}

#[test]
fn rejects_wrong_protocol_first() {
    let mut frame = register_server();
    frame.protocol = COORDINATOR_PROTOCOL_VERSION + 1;
    assert_eq!(
        validate_register_server(&frame).unwrap_err(),
        "Unsupported coordinator protocol"
    );
    let mut frame = register_peer();
    frame.protocol = COORDINATOR_PROTOCOL_VERSION - 1;
    assert_eq!(
        validate_register_peer(&frame, &[]).unwrap_err(),
        "Unsupported coordinator protocol"
    );
}

#[test]
fn rejects_empty_registration_fields_with_upstream_errors() {
    let mut frame = register_server();
    frame.server_connection_id = String::new();
    assert_eq!(
        validate_register_server(&frame).unwrap_err(),
        "Coordinator serverConnectionId must be a string"
    );
    let mut frame = register_server();
    frame.endpoint = String::new();
    assert_eq!(
        validate_register_server(&frame).unwrap_err(),
        "Coordinator endpoint must be a string"
    );
    let mut frame = register_peer();
    frame.peer_id = String::new();
    assert_eq!(
        validate_register_peer(&frame, &[]).unwrap_err(),
        "Coordinator peerId must be a string"
    );
}

#[test]
fn rejects_duplicate_and_reserved_peer_ids() {
    let frame = register_peer();
    assert_eq!(
        validate_register_peer(&frame, &["worker-abc".to_string()]).unwrap_err(),
        "Coordinator peer is already connected: worker-abc"
    );
    let mut reserved = register_peer();
    reserved.peer_id = "server".to_string();
    assert_eq!(
        validate_register_peer(&reserved, &[]).unwrap_err(),
        "Coordinator peer is already connected: server"
    );
}

#[test]
fn peer_registered_reply_matches_node_byte_oracle() {
    // Oracle: encodeControlLine[5] and [6].
    assert_eq!(
        encode(&peer_registered_reply("worker-abc", Some("srv-1"))),
        "{\"type\":\"peer_registered\",\"peerId\":\"worker-abc\",\"serverConnectionId\":\"srv-1\"}"
    );
    assert_eq!(
        encode(&peer_registered_reply("worker-abc", None)),
        "{\"type\":\"peer_registered\",\"peerId\":\"worker-abc\"}"
    );
    // Oracle: encodeControlLine[7].
    assert_eq!(
        encode(&server_registered_reply(
            "srv-1",
            &["a".to_string(), "b".to_string()]
        )),
        "{\"type\":\"server_registered\",\"serverConnectionId\":\"srv-1\",\"peers\":[\"a\",\"b\"]}"
    );
}

#[test]
fn coordinator_message_frames_round_trip_with_wire_names() {
    let message = CoordinatorMessage::ServerRegistered {
        server_connection_id: "srv-1".to_string(),
        peers: vec!["worker-abc".to_string()],
    };
    assert_eq!(
        encode(&message),
        "{\"type\":\"server_registered\",\"serverConnectionId\":\"srv-1\",\"peers\":[\"worker-abc\"]}"
    );
    let decoded: CoordinatorMessage =
        serde_json::from_str("{\"type\":\"peer_disconnected\",\"peerId\":\"worker-abc\"}").unwrap();
    assert_eq!(
        decoded,
        CoordinatorMessage::PeerDisconnected {
            peer_id: "worker-abc".to_string()
        }
    );
    let decoded: CoordinatorMessage = serde_json::from_str(
        "{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"shutdown\"}}",
    )
    .unwrap();
    assert_eq!(
        decoded,
        CoordinatorMessage::Message {
            from: "server".to_string(),
            payload: serde_json::json!({ "type": "shutdown" })
        }
    );
}

#[test]
fn validates_send_targets() {
    assert!(validate_send_target("worker-abc").is_ok());
    assert_eq!(
        validate_send_target("").unwrap_err(),
        "Coordinator message target must be a string"
    );
}

fn encode(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap()
}

// ── D1: routing core (oracle transcript from the live upstream
// coordinator, captured with node --experimental-strip-types) ───────────────

use super::server::{CoordinatorRouter, RouterAction};
use super::transport::{ControlConnector as _, MemoryConnector, MemoryHub};
use std::sync::Arc;

fn route(router: &mut CoordinatorRouter, connection: &str, line: &str) -> Vec<Value> {
    let actions = router.handle_line(connection, line).expect("routed");
    actions
        .into_iter()
        .map(|action| match action {
            RouterAction::Send { message, .. } => message,
            RouterAction::ClosePublic => serde_json::json!({ "action": "close_public" }),
        })
        .collect()
}

#[test]
fn router_server_registered_reply_matches_oracle_bytes() {
    let mut router = CoordinatorRouter::new();
    let out = route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-1","endpoint":"endpoint.sock"}"#,
    );
    assert_eq!(
        encode(&out),
        r#"[{"type":"server_registered","serverConnectionId":"server-conn-1","peers":[]}]"#
    );
}

#[test]
fn router_peer_registration_matches_oracle_bytes() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-1","endpoint":"endpoint.sock"}"#,
    );
    let out = route(
        &mut router,
        "c2",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    // peer_registered includes serverConnectionId while a server is live,
    // and the current server receives peer_connected (node oracle o4.mjs).
    assert_eq!(
        encode(&out),
        r#"[{"type":"peer_registered","peerId":"worker-1","serverConnectionId":"server-conn-1"},{"type":"peer_connected","peerId":"worker-1"}]"#
    );
}

#[test]
fn router_broadcast_reaches_peers_with_server_from() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-1","endpoint":"endpoint.sock"}"#,
    );
    route(
        &mut router,
        "c2",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    let out = route(
        &mut router,
        "c1",
        r#"{"type":"broadcast","payload":{"hello":"world"}}"#,
    );
    assert_eq!(
        encode(&out),
        r#"[{"type":"message","from":"server","payload":{"hello":"world"}}]"#
    );
    // A peer cannot broadcast (node oracle: "Only the current server may
    // broadcast").
    let error = router
        .handle_line("c2", r#"{"type":"broadcast","payload":{}}"#)
        .unwrap_err();
    assert_eq!(error, "Only the current server may broadcast");
}

#[test]
fn router_send_routes_to_named_peer() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-1","endpoint":"endpoint.sock"}"#,
    );
    route(
        &mut router,
        "c2",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    let out = route(
        &mut router,
        "c2",
        r#"{"type":"send","to":"server","payload":{"ping":1}}"#,
    );
    assert_eq!(
        encode(&out),
        r#"[{"type":"message","from":"worker-1","payload":{"ping":1}}]"#
    );
}

#[test]
fn router_replacement_sequence_matches_oracle_order() {
    // node oracle (o4.mjs): on the second register_server the peers receive
    // server_disconnected(prev) then server_connected(next), the previous
    // server socket receives server_replaced, and public connections close.
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "s1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-1","endpoint":"endpoint.sock"}"#,
    );
    route(
        &mut router,
        "p1",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    let out = route(
        &mut router,
        "s2",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"server-conn-2","endpoint":"endpoint.sock"}"#,
    );
    assert_eq!(
        encode(&out),
        r#"[{"type":"server_registered","serverConnectionId":"server-conn-2","peers":["worker-1"]},{"action":"close_public"},{"type":"server_disconnected","serverConnectionId":"server-conn-1"},{"type":"server_replaced"},{"type":"server_connected","serverConnectionId":"server-conn-2"}]"#
    );
    // The replacing server's server_registered already carried the live
    // peer set (node oracle: SERVER2-PEERS ["worker-1","worker-2"] shape).
    assert_eq!(router.server_connection_id(), Some("server-conn-2"));
    assert!(router.peer_ids().contains(&"worker-1".to_string()));
}

#[test]
fn router_peer_registered_without_server_omits_connection_id() {
    // node oracle frame 6: peer_registered carries no serverConnectionId
    // while no server is connected.
    let mut router = CoordinatorRouter::new();
    let out = route(
        &mut router,
        "c1",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    assert_eq!(
        encode(&out),
        r#"[{"type":"peer_registered","peerId":"worker-1"}]"#
    );
}

#[test]
fn router_bad_protocol_destroys_connection() {
    let mut router = CoordinatorRouter::new();
    let error = router
        .handle_line(
            "c1",
            r#"{"type":"register_peer","protocol":2,"peerId":"worker-9"}"#,
        )
        .unwrap_err();
    assert_eq!(error, "Unsupported coordinator protocol");
}

#[test]
fn router_unregistered_role_error() {
    let mut router = CoordinatorRouter::new();
    let error = router
        .handle_line("c1", r#"{"type":"send","to":"server","payload":1}"#)
        .unwrap_err();
    assert_eq!(error, "Coordinator connection did not register a role");
}

#[test]
fn router_duplicate_peer_rejected_with_exact_text() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"s","endpoint":"e"}"#,
    );
    route(
        &mut router,
        "c2",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    let error = router
        .handle_line(
            "c3",
            r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
        )
        .unwrap_err();
    assert_eq!(error, "Coordinator peer is already connected: worker-1");
    let error = router
        .handle_line(
            "c3",
            r#"{"type":"register_peer","protocol":3,"peerId":"server"}"#,
        )
        .unwrap_err();
    assert_eq!(error, "Coordinator peer is already connected: server");
}

#[test]
fn router_unknown_routing_message_error() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"s","endpoint":"e"}"#,
    );
    let error = router
        .handle_line("c1", r#"{"type":"multicast","payload":{}}"#)
        .unwrap_err();
    assert_eq!(error, "Unknown coordinator routing message: multicast");
}

#[test]
fn router_disconnect_notifies_server() {
    let mut router = CoordinatorRouter::new();
    route(
        &mut router,
        "c1",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"s","endpoint":"e"}"#,
    );
    route(
        &mut router,
        "c2",
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
    );
    let actions = router.handle_disconnect("c2");
    assert_eq!(
        encode(&actions),
        r#"[{"connection":"c1","message":{"type":"peer_disconnected","peerId":"worker-1"}}]"#
    );
}

// ── D1: loopback integration over the in-memory transport ─────────────────

fn read_line(socket: &mut dyn super::transport::ControlSocket) -> String {
    match super::transport::read_line_capped(socket, "Coordinator message is too large") {
        Ok(Some(line)) => line,
        Ok(None) => panic!("peer socket closed"),
        Err(error) => panic!("peer read failed: {error}"),
    }
}

fn memory_server() -> (
    Arc<super::server::CoordinatorServer>,
    Arc<MemoryHub>,
    MemoryConnector,
) {
    let hub = Arc::new(MemoryHub::new());
    let control_listener: Box<dyn super::transport::ControlListener> =
        Box::new(hub.bind("control").unwrap());
    let public_listener: Box<dyn super::transport::ControlListener> =
        Box::new(hub.bind("public").unwrap());
    let connector = MemoryConnector::new(Arc::clone(&hub));
    let handle = super::server::CoordinatorServer::bind(
        control_listener,
        public_listener,
        Arc::new(MemoryConnector::new(Arc::clone(&hub))),
    );
    (handle, hub, connector)
}

#[test]
fn loopback_server_and_peer_route_messages() {
    bounded_thread(loopback_server_and_peer_route_messages_case);
}

fn loopback_server_and_peer_route_messages_case() {
    let (server_handle, _hub, connector) = memory_server();
    assert!(server_handle.is_idle());

    let server = super::server::CoordinatorConnection::new(
        "control",
        "public",
        Some("server-conn-1".to_owned()),
    );
    let events = server.event_channel();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(async { server.connect(&connector).await })
        .unwrap();
    assert_eq!(server.server_connection_id, "server-conn-1");

    // Raw peer over the same hub (upstream o3/o4 oracle peer socket).
    let mut peer = connector.connect("control").unwrap();
    peer.write_line(concat!(
        r#"{"type":"register_peer","protocol":3,"peerId":"worker-1"}"#,
        "\n"
    ))
    .unwrap();
    assert_eq!(
        read_line(peer.as_mut()),
        r#"{"type":"peer_registered","peerId":"worker-1","serverConnectionId":"server-conn-1"}"#
    );

    let connected = events
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("peer registration event");
    assert!(matches!(connected,
        CoordinatorConnectionEvent::PeerConnected { peer_id } if peer_id == "worker-1"));

    // Server broadcast reaches the peer; peer send reaches the server.
    runtime
        .block_on(async {
            server
                .broadcast(serde_json::json!({ "hello": "world" }))
                .await
        })
        .unwrap();
    assert_eq!(
        read_line(peer.as_mut()),
        r#"{"type":"message","from":"server","payload":{"hello":"world"}}"#
    );
    peer.write_line(concat!(
        r#"{"type":"send","to":"server","payload":{"ping":1}}"#,
        "\n"
    ))
    .unwrap();
    let event = events
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("server event");
    match event {
        CoordinatorConnectionEvent::Message { from, payload } => {
            assert_eq!(from, "worker-1");
            assert_eq!(payload, serde_json::json!({ "ping": 1 }));
        }
        other => panic!("unexpected event {other:?}"),
    }

    // Disconnect notification reaches the current server.
    drop(peer);
    let event = events
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("disconnect event");
    assert!(matches!(event,
        CoordinatorConnectionEvent::PeerDisconnected { peer_id } if peer_id == "worker-1"));
    server_handle.shutdown();
}

#[test]
fn loopback_server_replacement_marks_replaced() {
    bounded_thread(loopback_server_replacement_marks_replaced_case);
}

fn loopback_server_replacement_marks_replaced_case() {
    let (server_handle, _hub, connector) = memory_server();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let first = super::server::CoordinatorConnection::new(
        "control",
        "public",
        Some("server-conn-1".to_owned()),
    );
    runtime
        .block_on(async { first.connect(&connector).await })
        .unwrap();
    let second = super::server::CoordinatorConnection::new(
        "control",
        "public",
        Some("server-conn-2".to_owned()),
    );
    runtime
        .block_on(async { second.connect(&connector).await })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !first.was_replaced() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(first.was_replaced());
    server_handle.shutdown();
}

#[test]
fn ensure_coordinator_reports_exited_child() {
    use crate::coding_agent::experimental::process::{
        InternalProcessChild, InternalProcessRole, ProcessSpawner,
    };
    struct ExitedSpawner;
    impl ProcessSpawner for ExitedSpawner {
        fn spawn(
            &self,
            _role: InternalProcessRole,
            _args: &[String],
            _extra_env: &[(String, String)],
        ) -> std::io::Result<Box<dyn InternalProcessChild>> {
            Ok(Box::new(ExitedChild))
        }
    }
    struct ExitedChild;
    impl InternalProcessChild for ExitedChild {
        fn pid(&self) -> Option<u32> {
            Some(1)
        }
        fn kill(&self) {}
        fn has_exited(&self) -> bool {
            true
        }
        fn wait_exit(&self) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }
    let hub = Arc::new(MemoryHub::new());
    let connector = MemoryConnector::new(Arc::clone(&hub));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime.block_on(async {
        super::server::ensure_coordinator("public", "control", &ExitedSpawner, &connector)
            .await
            .unwrap_err()
    });
    assert_eq!(error, "Coordinator exited during startup");
}

// Blocking transport failures must become bounded failures, not hang all gates.
fn bounded_thread(work: impl FnOnce() + Send + 'static) {
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)));
    });
    if let Err(panic) = recv
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("coordinator operation deadlocked")
    {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn memory_stream_frames_preserve_newlines_and_drop_unterminated_eof() {
    bounded_thread(|| {
        let hub = Arc::new(MemoryHub::new());
        let listener = hub.bind("framing").unwrap();
        let mut client = MemoryConnector::new(hub).connect("framing").unwrap();
        let mut server = super::transport::ControlListener::accept(&listener).unwrap();
        client.write_line("first\nsecond\r\npartial").unwrap();
        drop(client);
        assert_eq!(read_line(server.as_mut()), "first");
        assert_eq!(read_line(server.as_mut()), "second");
        assert!(
            super::transport::read_line_capped(server.as_mut(), "too large")
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn memory_clone_drop_retains_connection_and_shutdown_wakes_readers() {
    bounded_thread(|| {
        let hub = Arc::new(MemoryHub::new());
        let listener = hub.bind("halves").unwrap();
        let mut client = MemoryConnector::new(hub).connect("halves").unwrap();
        let mut server = super::transport::ControlListener::accept(&listener).unwrap();
        let mut reader = server.try_clone().unwrap();
        drop(server.try_clone().unwrap());
        client.write_line("still here\n").unwrap();
        assert_eq!(read_line(reader.as_mut()), "still here");
        let blocked = std::thread::spawn(move || reader.read_line().unwrap());
        server.shutdown();
        assert_eq!(blocked.join().unwrap(), None);
    });
}

#[test]
fn shutdown_closes_both_listeners_without_an_incoming_connection() {
    bounded_thread(|| {
        let (server, _hub, connector) = memory_server();
        server.shutdown();
        assert!(connector.connect("control").is_err());
        assert!(connector.connect("public").is_err());
    });
}

#[cfg(windows)]
#[test]
fn real_loopback_tcp_listener_close_unblocks_accept_and_releases_port() {
    bounded_thread(|| {
        use super::transport::{ControlListener, TcpControlListener};
        let listener = Arc::new(TcpControlListener::bind_ephemeral().unwrap());
        let path = listener.path().to_string();
        let waiting = listener.clone();
        let reader = std::thread::spawn(move || waiting.accept().is_err());
        listener.close();
        assert!(reader.join().unwrap());
        let rebound = TcpControlListener::bind(&path).unwrap();
        rebound.close();
    });
}

// Full transport regressions for the upstream publicConnections lifecycle.
struct ShutdownServer(Arc<super::server::CoordinatorServer>);
impl Drop for ShutdownServer {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}
fn eventually(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "coordinator state did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn raw_peer(connector: &MemoryConnector, id: &str) -> Box<dyn super::transport::ControlSocket> {
    let mut peer = connector.connect("control").unwrap();
    peer.write_line(
        &(serde_json::json!({"type":"register_peer", "protocol":3, "peerId":id}).to_string()
            + "\n"),
    )
    .unwrap();
    let reply: Value = serde_json::from_str(&read_line(peer.as_mut())).unwrap();
    assert_eq!(reply["type"], "peer_registered");
    peer
}
fn receive_bytes(socket: &mut dyn super::transport::ControlSocket, expected: &[u8]) {
    let mut bytes = vec![0; expected.len()];
    let mut offset = 0;
    while offset < bytes.len() {
        let n = socket.read_bytes(&mut bytes[offset..]).unwrap();
        assert!(n > 0, "proxy closed before delivering all bytes");
        offset += n;
    }
    assert_eq!(bytes, expected);
}
fn public_pair(
    connector: &MemoryConnector,
    listener: &dyn super::transport::ControlListener,
) -> (
    Box<dyn super::transport::ControlSocket>,
    Box<dyn super::transport::ControlSocket>,
) {
    let mut client = connector.connect("public").unwrap();
    let mut upstream = listener.accept().unwrap();
    client.write_all_bytes(b"\0\xffclient\n\r").unwrap();
    receive_bytes(upstream.as_mut(), b"\0\xffclient\n\r");
    upstream.write_all_bytes(b"\xfe\0upstream\n").unwrap();
    receive_bytes(client.as_mut(), b"\xfe\0upstream\n");
    (client, upstream)
}
fn assert_eof(socket: &mut dyn super::transport::ControlSocket) {
    assert_eq!(socket.read_bytes(&mut [0; 1]).unwrap(), 0);
}

#[test]
fn router_and_connection_preserve_peer_insertion_order() {
    let mut router = CoordinatorRouter::new();
    for peer in ["z", "2", "a"] {
        router
            .handle_line(
                peer,
                &serde_json::json!({"type":"register_peer","protocol":3,"peerId":peer}).to_string(),
            )
            .unwrap();
    }
    assert_eq!(router.peer_ids(), ["z", "2", "a"]);
    router.handle_disconnect("2");
    router
        .handle_line("2", r#"{"type":"register_peer","protocol":3,"peerId":"2"}"#)
        .unwrap();
    let registered = route(
        &mut router,
        "s",
        r#"{"type":"register_server","protocol":3,"serverConnectionId":"srv","endpoint":"endpoint"}"#,
    );
    assert_eq!(registered[0]["peers"], serde_json::json!(["z", "a", "2"]));
    let actions = router
        .handle_line("s", r#"{"type":"broadcast","payload":1}"#)
        .unwrap();
    let targets: Vec<_> = actions
        .iter()
        .map(|action| match action {
            RouterAction::Send { connection, .. } => connection.as_str(),
            RouterAction::ClosePublic => panic!("unexpected close"),
        })
        .collect();
    assert_eq!(targets, ["z", "a", "2"]);
    bounded_thread(|| {
        let (server, _hub, connector) = memory_server();
        let _cleanup = ShutdownServer(server);
        let mut peers: Vec<_> = ["z", "2", "a"]
            .into_iter()
            .map(|id| raw_peer(&connector, id))
            .collect();
        let connection =
            super::server::CoordinatorConnection::new("control", "endpoint", Some("srv".into()));
        let events = connection.event_channel();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(connection.connect(&connector)).unwrap();
        assert_eq!(connection.peer_ids(), ["z", "2", "a"]);
        drop(peers.remove(1));
        assert_eq!(
            events
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            CoordinatorConnectionEvent::PeerDisconnected {
                peer_id: "2".into()
            }
        );
        peers.push(raw_peer(&connector, "2"));
        assert_eq!(
            events
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            CoordinatorConnectionEvent::PeerConnected {
                peer_id: "2".into()
            }
        );
        assert_eq!(connection.peer_ids(), ["z", "a", "2"]);
        connection.close();
    });
}

#[test]
fn replacement_and_shutdown_close_every_real_public_proxy() {
    bounded_thread(|| {
        let (handle, hub, connector) = memory_server();
        let _cleanup = ShutdownServer(handle.clone());
        let first_listener = hub.bind("first-endpoint").unwrap();
        let second_listener = hub.bind("second-endpoint").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let first = super::server::CoordinatorConnection::new(
            "control",
            "first-endpoint",
            Some("first".into()),
        );
        runtime.block_on(first.connect(&connector)).unwrap();
        let (mut c1, mut u1) = public_pair(&connector, &first_listener);
        let (mut c2, mut u2) = public_pair(&connector, &first_listener);
        eventually(|| handle.connection_counts() == (1, 2));
        let second = super::server::CoordinatorConnection::new(
            "control",
            "second-endpoint",
            Some("second".into()),
        );
        runtime.block_on(second.connect(&connector)).unwrap();
        eventually(|| first.was_replaced());
        for socket in [&mut c1, &mut u1, &mut c2, &mut u2] {
            assert_eof(socket.as_mut());
        }
        eventually(|| handle.connection_counts() == (2, 0));
        let (mut c3, mut u3) = public_pair(&connector, &second_listener);
        handle.shutdown();
        assert_eof(c3.as_mut());
        assert_eof(u3.as_mut());
        assert_eq!(handle.connection_counts(), (0, 0));
    });
}

#[test]
fn one_proxy_finishing_does_not_hide_other_proxies_or_unregistered_controls() {
    bounded_thread(|| {
        let (handle, hub, connector) = memory_server();
        let _cleanup = ShutdownServer(handle.clone());
        let lease = connector.connect("control").unwrap();
        eventually(|| handle.connection_counts() == (1, 0));
        assert!(
            !handle.is_idle(),
            "unregistered startup leases count as live controls"
        );
        drop(lease);
        eventually(|| handle.is_idle());
        let listener = hub.bind("endpoint").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let server =
            super::server::CoordinatorConnection::new("control", "endpoint", Some("srv".into()));
        runtime.block_on(server.connect(&connector)).unwrap();
        let (c1, mut u1) = public_pair(&connector, &listener);
        let (c2, mut u2) = public_pair(&connector, &listener);
        server.close();
        eventually(|| handle.connection_counts() == (0, 2));
        drop(c1);
        assert_eof(u1.as_mut());
        eventually(|| handle.connection_counts() == (0, 1));
        assert!(!handle.is_idle());
        drop(c2);
        assert_eof(u2.as_mut());
        eventually(|| handle.is_idle());
    });
}

#[test]
fn memory_socket_rejects_writes_after_the_remote_endpoint_closes() {
    bounded_thread(|| {
        use super::transport::ControlListener;
        for explicit in [false, true] {
            let hub = Arc::new(MemoryHub::new());
            let listener = hub.bind("write-close").unwrap();
            let mut client = MemoryConnector::new(hub).connect("write-close").unwrap();
            let mut remote = listener.accept().unwrap();
            if explicit {
                remote.shutdown();
            } else {
                drop(remote);
            }
            assert_eq!(
                client.write_all_bytes(b"late").unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
        }
    });
}

#[test]
fn replacement_closes_pending_clients_and_rejects_late_old_endpoint_connections() {
    bounded_thread(|| {
        use super::transport::{ControlConnector, ControlListener, ControlSocket};
        use std::sync::{mpsc, Mutex};
        struct GatedConnector {
            connector: MemoryConnector,
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl ControlConnector for GatedConnector {
            fn connect(&self, path: &str) -> std::io::Result<Box<dyn ControlSocket>> {
                if path == "old-endpoint" {
                    self.entered.send(()).unwrap();
                    self.release
                        .lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .map_err(std::io::Error::other)?;
                }
                self.connector.connect(path)
            }
        }
        let hub = Arc::new(MemoryHub::new());
        let old_endpoint = hub.bind("old-endpoint").unwrap();
        let connector = MemoryConnector::new(hub.clone());
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let handle = super::server::CoordinatorServer::bind(
            Box::new(hub.bind("control").unwrap()),
            Box::new(hub.bind("public").unwrap()),
            Arc::new(GatedConnector {
                connector: MemoryConnector::new(hub),
                entered,
                release: Mutex::new(released),
            }),
        );
        let _cleanup = ShutdownServer(handle.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let old = super::server::CoordinatorConnection::new(
            "control",
            "old-endpoint",
            Some("old".into()),
        );
        runtime.block_on(old.connect(&connector)).unwrap();
        let mut client = connector.connect("public").unwrap();
        entry
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(handle.connection_counts(), (1, 1));
        let new = super::server::CoordinatorConnection::new(
            "control",
            "new-endpoint",
            Some("new".into()),
        );
        runtime.block_on(new.connect(&connector)).unwrap();
        eventually(|| old.was_replaced());
        assert_eof(client.as_mut());
        release.send(()).unwrap();
        let mut stale_upstream = old_endpoint.accept().unwrap();
        assert_eof(stale_upstream.as_mut());
        eventually(|| handle.connection_counts() == (2, 0));
    });
}

#[test]
fn shutdown_interrupts_a_blocked_control_write_without_waiting_for_its_lock() {
    bounded_thread(|| {
        use super::transport::{ControlListener, ControlSocket, MemoryListener};
        use std::sync::{mpsc, Condvar, Mutex};
        struct BlockState {
            entered: Mutex<Option<mpsc::Sender<()>>>,
            closed: Mutex<bool>,
            wake: Condvar,
        }
        struct BlockSocket {
            inner: Box<dyn ControlSocket>,
            state: Arc<BlockState>,
        }
        impl BlockSocket {
            fn block_write(&self) -> std::io::Result<()> {
                if let Some(send) = self.state.entered.lock().unwrap().take() {
                    send.send(()).unwrap();
                }
                let mut closed = self.state.closed.lock().unwrap();
                while !*closed {
                    closed = self.state.wake.wait(closed).unwrap();
                }
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed during write",
                ))
            }
        }
        impl ControlSocket for BlockSocket {
            fn read_line(&mut self) -> std::io::Result<Option<String>> {
                self.inner.read_line()
            }
            fn write_line(&mut self, _line: &str) -> std::io::Result<()> {
                self.block_write()
            }
            fn read_bytes(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.inner.read_bytes(bytes)
            }
            fn write_all_bytes(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
                self.block_write()
            }
            fn shutdown(&mut self) {
                self.inner.shutdown();
                *self.state.closed.lock().unwrap() = true;
                self.state.wake.notify_all();
            }
            fn try_clone(&self) -> Option<Box<dyn ControlSocket>> {
                Some(Box::new(Self {
                    inner: self.inner.try_clone()?,
                    state: self.state.clone(),
                }))
            }
        }
        struct BlockListener {
            inner: MemoryListener,
            state: Arc<BlockState>,
        }
        impl ControlListener for BlockListener {
            fn accept(&self) -> std::io::Result<Box<dyn ControlSocket>> {
                Ok(Box::new(BlockSocket {
                    inner: self.inner.accept()?,
                    state: self.state.clone(),
                }))
            }
            fn path(&self) -> &str {
                self.inner.path()
            }
            fn close(&self) {
                self.inner.close();
            }
        }
        let hub = Arc::new(MemoryHub::new());
        let connector = MemoryConnector::new(hub.clone());
        let (send, entered) = mpsc::channel();
        let state = Arc::new(BlockState {
            entered: Mutex::new(Some(send)),
            closed: Mutex::new(false),
            wake: Condvar::new(),
        });
        let handle = super::server::CoordinatorServer::bind(
            Box::new(BlockListener {
                inner: hub.bind("control").unwrap(),
                state,
            }),
            Box::new(hub.bind("public").unwrap()),
            Arc::new(MemoryConnector::new(hub)),
        );
        let _cleanup = ShutdownServer(handle.clone());
        let mut client = connector.connect("control").unwrap();
        client
            .write_line("{\"type\":\"register_peer\",\"protocol\":3,\"peerId\":\"blocked\"}\n")
            .unwrap();
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        handle.shutdown();
        assert_eof(client.as_mut());
        assert_eq!(handle.connection_counts(), (0, 0));
    });
}

// ── D1 completion: empty-shutdown timer, socket hygiene, process entry ─────

/// Serializes the process-global `runCoordinatorProcess` `running` flag
/// across parallel tests (upstream the flag lives in a single process that
/// runs one coordinator).
static RUN_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn run_coordinator_process_rejects_bad_arguments_and_reentry() {
    let _guard = RUN_GUARD.lock().unwrap();
    assert_eq!(
        super::server::run_coordinator_process(&[]).unwrap_err(),
        "Coordinator requires public and control socket paths"
    );
    assert_eq!(
        super::server::run_coordinator_process(&["only-one".to_string()]).unwrap_err(),
        "Coordinator requires public and control socket paths"
    );
}

#[test]
fn run_coordinator_process_starts_embedded_server_and_reports_reentry() {
    let _guard = RUN_GUARD.lock().unwrap();
    let unique = format!(
        "pi-coord-entry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(&unique);
    std::fs::create_dir_all(&dir).unwrap();
    let public_path = dir.join("p.sock");
    let control_path = dir.join("c.sock");
    let server = super::server::run_coordinator_process(&[
        public_path.to_string_lossy().into_owned(),
        control_path.to_string_lossy().into_owned(),
    ])
    .unwrap_or_else(|error| panic!("embedded coordinator start failed: {error}"));
    assert!(server.is_idle());
    assert_eq!(
        super::server::run_coordinator_process(&[
            public_path.to_string_lossy().into_owned(),
            control_path.to_string_lossy().into_owned(),
        ])
        .unwrap_err(),
        "Coordinator process is already running"
    );
    server.shutdown();
    assert!(std::fs::remove_dir_all(&dir).is_ok());
}

#[test]
fn empty_shutdown_watchdog_fires_after_the_startup_grace() {
    bounded_thread(|| {
        let (server, _hub, connector) = memory_server();
        let _cleanup = ShutdownServer(server.clone());
        server.start_empty_shutdown_watchdog(
            std::time::Duration::from_millis(150),
            std::time::Duration::from_millis(50),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if connector.connect("control").is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "empty coordinator was never shut down by the watchdog"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

#[test]
fn empty_shutdown_watchdog_honors_checkempty_rearm_and_cancellation() {
    bounded_thread(|| {
        let (server, _hub, connector) = memory_server();
        let _cleanup = ShutdownServer(server.clone());
        server.start_empty_shutdown_watchdog(
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(80),
        );
        // A registration cancels the startup grace (upstream
        // `registerPeer` -> `cancelEmptyShutdown`).
        let peer = raw_peer(&connector, "watchdog-peer");
        std::thread::sleep(std::time::Duration::from_millis(320));
        assert!(
            connector.connect("control").is_ok(),
            "live peer must keep the coordinator alive past the startup grace"
        );
        // Disconnecting re-arms the shorter `checkEmpty` grace.
        drop(peer);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if connector.connect("control").is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "coordinator did not retire after the empty grace"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

#[test]
fn connection_close_does_not_resolve_the_replaced_promise() {
    bounded_thread(|| {
        let (_server, _hub, connector) = memory_server();
        let connection =
            super::server::CoordinatorConnection::new("control", "public", Some("srv".into()));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(connection.connect(&connector)).unwrap();
        connection.close();
        // Upstream `#disconnected` early-returns once closed, so `replaced`
        // stays pending and `wasReplaced` stays false.
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(!connection.was_replaced());
        let replaced = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), connection.replaced()).await
        });
        assert!(replaced.is_err(), "close() must not resolve replaced");
    });
}

#[cfg(unix)]
#[test]
fn stale_socket_hygiene_matches_upstream_error_text() {
    let unique = format!(
        "pi-coord-hygiene-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(&unique);
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("missing.sock");
    let regular = dir.join("regular.sock");
    std::fs::write(&regular, b"not a socket").unwrap();
    assert_eq!(
        super::server::remove_stale_socket(missing.to_str().unwrap()).unwrap(),
        ()
    );
    assert_eq!(
        super::server::remove_stale_socket(regular.to_str().unwrap()).unwrap_err(),
        format!(
            "Coordinator path is not a socket: {}",
            regular.to_str().unwrap()
        )
    );

    // A live socket refuses replacement.
    let live = dir.join("live.sock");
    let live_listener =
        super::transport::UnixControlListener::bind(live.to_str().unwrap()).unwrap();
    assert_eq!(
        super::server::remove_stale_socket(live.to_str().unwrap()).unwrap_err(),
        format!(
            "Coordinator socket is already active: {}",
            live.to_str().unwrap()
        )
    );

    // A dead socket file is removed so the next bind succeeds.
    let stale = dir.join("stale.sock");
    {
        let _listener = std::os::unix::net::UnixListener::bind(&stale).unwrap();
    }
    super::server::remove_stale_socket(stale.to_str().unwrap()).unwrap();
    let rebound = super::transport::UnixControlListener::bind(stale.to_str().unwrap()).unwrap();
    drop(rebound);
    drop(live_listener);
    super::server::cleanup_socket(live.to_str().unwrap()).unwrap();

    // `restrictSocket` tightens the socket path to owner-only.
    use std::os::unix::fs::PermissionsExt;
    let guarded = dir.join("guarded.sock");
    std::fs::write(&guarded, b"").unwrap();
    std::fs::set_permissions(&guarded, std::fs::Permissions::from_mode(0o644)).unwrap();
    super::server::restrict_socket(guarded.to_str().unwrap()).unwrap();
    let mode = std::fs::metadata(&guarded).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert!(std::fs::remove_dir_all(&dir).is_ok());
}

// ── D1 + D4: a real detached coordinator child over the platform transport ─
//
// Upstream authority: coordinator.ts
// (sha256 c65c9b03ab980d12b4a0bf938b39af7462f40a79d4a4331f94cda9df0ea12b62)
// plus experimental-internal-process.test.ts
// (sha256 278bac53861ae1e9aed7f14224ee03ef68ee7dfb8a1ba50d072cbf8f0babc3ab)
// "starts the coordinator through the current runtime" and "waits for a
// failed activation child to terminate" (upstream `skipIf(process.platform
// === "win32")` over Unix sockets; the port's Windows loopback-TCP transport
// makes the scenario runnable here, with the socket-path face confined to
// the transport).

/// The fixture the child process runs: gated by env so a normal test run
/// no-ops. Mirrors upstream's coordinator entry block (`consumeInternalProcessRole`
/// then `runCoordinatorProcess(process.argv.slice(2))`), with the argv pair
/// carried in env because libtest owns argv.
#[test]
fn coordinator_child_fixture() {
    let Some(paths) = std::env::var_os(COORDINATOR_FIXTURE_ENV) else {
        return;
    };
    std::env::remove_var(COORDINATOR_FIXTURE_ENV);
    let paths: Vec<String> = serde_json::from_str(paths.to_str().expect("fixture paths are UTF-8"))
        .expect("fixture paths JSON");
    // Keep the server alive for the process lifetime (upstream: the
    // coordinator lives until `shutdownCoordinator` / `process.exit`).
    let _server = super::server::run_coordinator_process(&paths).expect("fixture coordinator");
    let (_park_tx, park_rx) = std::sync::mpsc::channel::<()>();
    let _ = park_rx.recv();
}

const COORDINATOR_FIXTURE_ENV: &str = "PI_RUST_COORDINATOR_CHILD_FIXTURE";

use std::sync::atomic::Ordering;

use crate::coding_agent::experimental::coordinator::transport::ControlListener;

/// Endpoint pair for one test run: real Unix socket paths below the sun_path
/// limit on POSIX, ephemeral loopback TCP ports on Windows (the port's
/// disclosed Windows transport).
fn coordinator_child_paths() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    #[cfg(unix)]
    {
        let public = dir.path().join("p.sock");
        let control = dir.path().join("c.sock");
        (
            dir,
            public.to_string_lossy().into_owned(),
            control.to_string_lossy().into_owned(),
        )
    }
    #[cfg(not(unix))]
    {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        (
            dir,
            super::transport::tcp_path(port),
            super::transport::tcp_path(port + 1),
        )
    }
}

#[test]
fn spawns_a_real_coordinator_child_and_terminates_it() {
    let (_dir, public_path, control_path) = coordinator_child_paths();
    let exe = std::env::current_exe().expect("test executable");
    // libtest omits the crate name from its test paths; the child runs only
    // the fixture test (the `--exact` filter), carrying the internal-role env
    // of the D4 spawn contract (`__PI_INTERNAL_SPAWN=coordinator`).
    let (_, module) = module_path!().split_once("::").unwrap();
    let mut command = std::process::Command::new(exe);
    command
        .args([
            "--exact",
            &format!("{module}::coordinator_child_fixture"),
            "--nocapture",
        ])
        .env(
            crate::coding_agent::experimental::process::INTERNAL_PROCESS_ENV,
            "coordinator",
        )
        .env(
            COORDINATOR_FIXTURE_ENV,
            serde_json::json!([public_path, control_path]).to_string(),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().expect("spawn detached coordinator child");
    let pid = child.id();
    assert_ne!(pid, std::process::id());

    // Upstream `expect.poll(() => canConnect(controlPath))`: the child binds
    // the control endpoint and accepts control connections.
    let connector = super::transport::platform_connector();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut lease = None;
    while lease.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "coordinator child never opened its control endpoint"
        );
        match super::server::try_connect(&*connector, &control_path) {
            Ok(Some(socket)) => lease = Some(socket),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                panic!("control connect failed: {error}");
            }
        }
    }
    drop(lease);

    // Upstream `terminateInternalProcess`: SIGKILL then the endpoint stops
    // accepting (upstream `signalCode === "SIGKILL"`; the raw face here is
    // `Child::kill`).
    let _ = child.kill();
    let _ = child.wait();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match super::server::try_connect(&*connector, &control_path) {
            Ok(None) => break,
            Ok(Some(socket)) => drop(socket),
            Err(error) => panic!("control connect failed: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "terminated coordinator still accepts control connections"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_coordinator_spawns_and_reuses_a_live_child() {
    use crate::coding_agent::experimental::process::{
        InternalProcessChild, InternalProcessRole, ProcessSpawner,
    };

    // A "child" that simulates the coordinator's startup: binds the control
    // endpoint a beat later, then accepts (and parks on) one connection.
    struct CoordinatorSimulatingSpawner {
        hub: Arc<MemoryHub>,
        spawns: Arc<std::sync::atomic::AtomicUsize>,
    }
    struct NeverExitingChild;
    impl InternalProcessChild for NeverExitingChild {
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        fn kill(&self) {}
        fn has_exited(&self) -> bool {
            false
        }
        fn wait_exit(&self) -> futures::future::BoxFuture<'_, ()> {
            Box::pin(std::future::pending())
        }
    }
    impl ProcessSpawner for CoordinatorSimulatingSpawner {
        fn spawn(
            &self,
            role: InternalProcessRole,
            args: &[String],
            _extra_env: &[(String, String)],
        ) -> std::io::Result<Box<dyn InternalProcessChild>> {
            assert_eq!(role, InternalProcessRole::Coordinator);
            assert_eq!(args.len(), 2);
            self.spawns.fetch_add(1, Ordering::SeqCst);
            let control_path = args[1].clone();
            let hub = Arc::clone(&self.hub);
            std::thread::Builder::new()
                .name("simulated-coordinator".to_owned())
                .spawn(move || {
                    let listener = hub.bind(&control_path).expect("simulated bind");
                    let _ = listener.accept();
                })
                .expect("spawn simulated coordinator");
            Ok(Box::new(NeverExitingChild))
        }
    }

    let hub = Arc::new(MemoryHub::new());
    let spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let spawner = CoordinatorSimulatingSpawner {
        hub: Arc::clone(&hub),
        spawns: Arc::clone(&spawns),
    };
    let connector = MemoryConnector::new(Arc::clone(&hub));

    // First connect attempt misses (ENOENT face), the spawned child binds,
    // the poll loop connects and hands back a lease.
    let lease = super::server::ensure_coordinator("public", "control", &spawner, &connector)
        .await
        .expect("lease");
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    lease.close();

    // A live coordinator is adopted without spawning a second child (the
    // simulated child's listener is still bound).
    let adopted = super::server::ensure_coordinator("public", "control", &spawner, &connector)
        .await
        .expect("adopted lease");
    adopted.close();
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}
