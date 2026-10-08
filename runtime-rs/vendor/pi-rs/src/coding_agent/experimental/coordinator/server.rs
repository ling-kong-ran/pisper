//! D1 coordinator process server: the deterministic routing core
//! (`registerServer` / `registerPeer` / `handleRoutedMessage` /
//! disconnect handling) plus the threaded accept shell, the client-side
//! `CoordinatorConnection` connector, the `ensureCoordinator` startup lease,
//! the socket-path hygiene (`removeStaleSocket`/`restrictSocket`/
//! `cleanupSocket`) and the `runCoordinatorProcess` entry with the upstream
//! empty-shutdown timer (`EMPTY_STARTUP_GRACE_MS`/`EMPTY_SHUTDOWN_GRACE_MS`).
//!
//! The routing core is a pure state machine over opaque connection ids so
//! the byte-exact behavior (reply frames, notification order, validation
//! error strings) is testable without any transport; the shell wires real
//! listeners/sockets to it. Threads replace the upstream event loop; frame
//! bytes come from [`encode_control_line`]. Platform seams remain explicit:
//! Windows uses loopback TCP, not named pipes; Unix socket behavior needs
//! Unix-host validation; upstream's SIGINT/SIGTERM handlers have no portable
//! `std` equivalent and stay embedder-owned (see `run_coordinator_process`).

use crate::coding_agent::extensions::types::OrderedMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::coding_agent::experimental::coordinator::transport::{
    read_line_capped, ControlConnector, ControlListener, ControlSocket,
};
use crate::coding_agent::experimental::coordinator::{
    validate_register_peer, validate_register_server, validate_send_target,
    CoordinatorConnectionEvent, CoordinatorMessage, RegisterPeerFrame, RegisterServerFrame,
    COORDINATOR_PROTOCOL_VERSION,
};
use crate::coding_agent::experimental::process::{
    encode_control_line, InternalProcessChild, InternalProcessRole, ProcessSpawner,
};

use serde::de::DeserializeOwned;

const COORDINATOR_START_TIMEOUT_MS: u64 = 10_000;
const COORDINATOR_RETRY_MS: u64 = 10;

/// Upstream `EMPTY_STARTUP_GRACE_MS`: the initial empty-shutdown timer armed
/// by `main()` right after listening.
pub const EMPTY_STARTUP_GRACE_MS: u64 = 30_000;
/// Upstream `EMPTY_SHUTDOWN_GRACE_MS`: the grace re-armed by `checkEmpty`
/// once the process has hosted at least one registration/public connection
/// and become empty again.
pub const EMPTY_SHUTDOWN_GRACE_MS: u64 = 250;

// ── Deterministic routing core ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    Server,
    Peer,
}

struct ServerEntry {
    connection: String,
    server_connection_id: String,
    endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum RouterAction {
    /// `writeRoutedLine(target, message)`; serialized by the shell.
    Send { connection: String, message: Value },
    /// `closePublicConnections()` (the shell drops its proxied pairs).
    ClosePublic,
}

/// The upstream coordinator process's control-connection state machine, as a
/// pure function of (state, connection id, incoming line). Connection ids are
/// opaque; the shell assigns them. Errors destroy the connection
/// (`socket.destroy(error)` upstream), success returns ordered actions.
#[derive(Default)]
pub struct CoordinatorRouter {
    roles: HashMap<String, Role>,
    peers: OrderedMap<String>,
    current_server: Option<ServerEntry>,
    public_open: bool,
}

impl CoordinatorRouter {
    pub fn new() -> Self {
        CoordinatorRouter::default()
    }

    pub fn server_connection_id(&self) -> Option<&str> {
        self.current_server
            .as_ref()
            .map(|server| server.server_connection_id.as_str())
    }

    pub fn server_endpoint(&self) -> Option<&str> {
        self.current_server
            .as_ref()
            .map(|server| server.endpoint.as_str())
    }

    pub fn has_server(&self) -> bool {
        self.current_server.is_some()
    }

    pub fn peer_ids(&self) -> Vec<String> {
        self.peers.keys().map(str::to_owned).collect()
    }

    /// Upstream `checkEmpty`'s "no live state" conjunction.
    pub fn is_empty(&self) -> bool {
        self.current_server.is_none()
            && self.peers.is_empty()
            && self.roles.is_empty()
            && !self.public_open
    }

    /// Shell hook for `acceptPublicConnection` bookkeeping.
    pub fn set_public_open(&mut self, open: bool) {
        self.public_open = open;
    }

    fn parse_control_message(line: &str) -> Result<Value, String> {
        let value: Value =
            serde_json::from_str(line).map_err(|_| "Coordinator sent invalid JSON".to_string())?;
        if value.get("type").and_then(Value::as_str).is_none() {
            return Err("Coordinator message must have a type".to_string());
        }
        Ok(value)
    }

    fn decode_frame<T: DeserializeOwned>(message: &Value) -> Result<T, String> {
        serde_json::from_value(message.clone())
            .map_err(|_| "Unsupported coordinator protocol".to_string())
    }

    /// Upstream `acceptControlConnection`'s routed-line handler.
    pub fn handle_line(
        &mut self,
        connection: &str,
        line: &str,
    ) -> Result<Vec<RouterAction>, String> {
        let message = Self::parse_control_message(line)?;
        let Some(role) = self.roles.get(connection).cloned() else {
            let message_type = message["type"].as_str().unwrap_or_default().to_owned();
            if message_type == "register_server" {
                let frame: RegisterServerFrame = Self::decode_frame(&message)?;
                return self.register_server(connection, &frame);
            }
            if message_type == "register_peer" {
                let frame: RegisterPeerFrame = Self::decode_frame(&message)?;
                return self.register_peer(connection, &frame);
            }
            return Err("Coordinator connection did not register a role".to_string());
        };
        if role == Role::Server {
            let is_current = self
                .current_server
                .as_ref()
                .is_some_and(|server| server.connection == connection);
            if is_current {
                return self.handle_routed_message("server", &message);
            }
            return Ok(Vec::new());
        }
        let peer_id = self
            .peers
            .iter()
            .find(|(_, conn)| conn.as_str() == connection)
            .map(|(peer, _)| peer.to_owned())
            .ok_or_else(|| "Coordinator connection did not register a role".to_string())?;
        self.handle_routed_message(&peer_id, &message)
    }

    fn register_server(
        &mut self,
        connection: &str,
        frame: &RegisterServerFrame,
    ) -> Result<Vec<RouterAction>, String> {
        let (server_connection_id, endpoint) = validate_register_server(frame)?;
        let previous = self.current_server.replace(ServerEntry {
            connection: connection.to_owned(),
            server_connection_id: server_connection_id.clone(),
            endpoint,
        });
        let replaced = previous
            .filter(|previous| previous.connection != connection)
            .map(|previous| (previous.connection, previous.server_connection_id));
        self.roles.insert(connection.to_owned(), Role::Server);
        // Upstream reply order: server_registered to the new socket; then
        // closePublicConnections + server_disconnected to peers +
        // server_replaced to the previous socket; then server_connected to
        // peers.
        let mut actions = vec![RouterAction::Send {
            connection: connection.to_owned(),
            message: crate::coding_agent::experimental::coordinator::server_registered_reply(
                &server_connection_id,
                &self.peer_ids(),
            ),
        }];
        if let Some((previous_connection, previous_server_connection_id)) = replaced {
            actions.push(RouterAction::ClosePublic);
            for peer_connection in self.peer_connections() {
                actions.push(RouterAction::Send {
                    connection: peer_connection,
                    message: json!({
                        "type": "server_disconnected",
                        "serverConnectionId": previous_server_connection_id,
                    }),
                });
            }
            actions.push(RouterAction::Send {
                connection: previous_connection,
                message: json!({ "type": "server_replaced" }),
            });
        }
        for peer_connection in self.peer_connections() {
            actions.push(RouterAction::Send {
                connection: peer_connection,
                message: json!({
                    "type": "server_connected",
                    "serverConnectionId": server_connection_id,
                }),
            });
        }
        Ok(actions)
    }

    fn register_peer(
        &mut self,
        connection: &str,
        frame: &RegisterPeerFrame,
    ) -> Result<Vec<RouterAction>, String> {
        let peer_id = validate_register_peer(frame, &self.peer_ids())?;
        self.peers.set(peer_id.clone(), connection.to_owned());
        self.roles.insert(connection.to_owned(), Role::Peer);
        let mut actions = vec![RouterAction::Send {
            connection: connection.to_owned(),
            message: crate::coding_agent::experimental::coordinator::peer_registered_reply(
                &peer_id,
                self.server_connection_id(),
            ),
        }];
        if let Some(server) = &self.current_server {
            actions.push(RouterAction::Send {
                connection: server.connection.clone(),
                message: json!({ "type": "peer_connected", "peerId": peer_id }),
            });
        }
        Ok(actions)
    }

    fn peer_connections(&self) -> Vec<String> {
        self.peers.values().cloned().collect()
    }

    fn handle_routed_message(
        &mut self,
        from: &str,
        message: &Value,
    ) -> Result<Vec<RouterAction>, String> {
        let message_type = message["type"].as_str().unwrap_or_default().to_owned();
        if message_type == "send" {
            let to = message["to"].as_str().unwrap_or_default();
            validate_send_target(to)?;
            let target = if to == "server" {
                self.current_server
                    .as_ref()
                    .map(|server| server.connection.clone())
            } else {
                self.peers.get(to).cloned()
            };
            if let Some(target) = target {
                return Ok(vec![RouterAction::Send {
                    connection: target,
                    message: json!({
                        "type": "message",
                        "from": from,
                        "payload": message["payload"].clone(),
                    }),
                }]);
            }
            return Ok(Vec::new());
        }
        if message_type == "broadcast" {
            if from != "server" {
                return Err("Only the current server may broadcast".to_string());
            }
            return Ok(self
                .peer_connections()
                .into_iter()
                .map(|connection| RouterAction::Send {
                    connection,
                    message: json!({
                        "type": "message",
                        "from": from,
                        "payload": message["payload"].clone(),
                    }),
                })
                .collect());
        }
        Err(format!(
            "Unknown coordinator routing message: {message_type}"
        ))
    }

    /// Upstream `acceptControlConnection`'s disconnect handler.
    pub fn handle_disconnect(&mut self, connection: &str) -> Vec<RouterAction> {
        let mut actions = Vec::new();
        self.roles.remove(connection);
        let was_current_server = self
            .current_server
            .as_ref()
            .is_some_and(|server| server.connection == connection);
        if was_current_server {
            if let Some(server) = self.current_server.take() {
                for peer_connection in self.peer_connections() {
                    actions.push(RouterAction::Send {
                        connection: peer_connection,
                        message: json!({
                            "type": "server_disconnected",
                            "serverConnectionId": server.server_connection_id,
                        }),
                    });
                }
            }
        }
        let removed_peer = self
            .peers
            .iter()
            .find(|(_, conn)| conn.as_str() == connection)
            .map(|(peer, _)| peer.to_owned());
        if let Some(peer_id) = removed_peer {
            self.peers.delete(&peer_id);
            if let Some(server) = &self.current_server {
                actions.push(RouterAction::Send {
                    connection: server.connection.clone(),
                    message: json!({ "type": "peer_disconnected", "peerId": peer_id }),
                });
            }
        }
        actions
    }
}

// ── Threaded server shell ──────────────────────────────────────────────────

#[derive(Clone)]
struct PublicPair {
    client: Arc<SharedSocket>,
    // Registered before connect starts, so replacement/shutdown can close
    // the client even while the upstream connection is still pending.
    upstream: Option<Arc<SharedSocket>>,
}

impl PublicPair {
    fn shutdown(&self) {
        self.client.shutdown();
        if let Some(upstream) = &self.upstream {
            upstream.shutdown();
        }
    }
}

struct ShellState {
    router: CoordinatorRouter,
    sockets: HashMap<String, Arc<SharedSocket>>,
    public_pairs: OrderedMap<PublicPair>,
    shutting_down: bool,
}

/// One running coordinator process (in-process server). Mirrors
/// `runCoordinatorProcess` plus the control/public accept loops; the
/// empty-shutdown timers (`EMPTY_STARTUP_GRACE_MS` /
/// `EMPTY_SHUTDOWN_GRACE_MS`) drive [`CoordinatorServer::shutdown`] from the
/// embedder via [`CoordinatorServer::is_idle`] exactly as upstream's
/// `checkEmpty` would.
impl std::fmt::Debug for CoordinatorServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Trait-object listeners/connectors are not Debug; the state summary
        // is the observable identity the tests assert on.
        f.debug_struct("CoordinatorServer").finish_non_exhaustive()
    }
}

pub struct CoordinatorServer {
    state: Arc<Mutex<ShellState>>,
    // Serialize each routing mutation together with its ordered actions.
    // Shutdown never waits on this lock: it must interrupt blocked writes.
    routing: Mutex<()>,
    control_listener: Arc<dyn ControlListener>,
    public_listener: Arc<dyn ControlListener>,
    connector: Arc<dyn ControlConnector>,
    shutdown_flag: Arc<AtomicBool>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// Upstream's single `emptyTimer` slot: an armed deadline plus the
    /// condvar the watchdog thread sleeps on.
    empty_timer: Arc<(Mutex<Option<Instant>>, std::sync::Condvar)>,
    /// The grace used by `checkEmpty` once the startup grace has been
    /// superseded (upstream keeps both constants fixed; tests shorten them).
    empty_grace: AtomicU64,
    /// Set when this server was created by [`run_coordinator_process`]:
    /// shutting down then clears the process-global `running` flag — the
    /// in-process face of upstream's `process.exit(0)` (a fresh coordinator
    /// process starts with `running === false`).
    process_entry: AtomicBool,
}

impl CoordinatorServer {
    /// Upstream `main()`'s listen face: bind the control and public listeners
    /// and start serving. Stale-socket hygiene (`removeStaleSocket`) and the
    /// empty-shutdown watchdog are separate: embedders either call
    /// [`run_coordinator_process`] (full upstream entry behavior) or bind
    /// pre-built listeners and own the lifecycle themselves.
    pub fn bind(
        control_listener: Box<dyn ControlListener>,
        public_listener: Box<dyn ControlListener>,
        connector: Arc<dyn ControlConnector>,
    ) -> Arc<CoordinatorServer> {
        let server = Arc::new(CoordinatorServer {
            state: Arc::new(Mutex::new(ShellState {
                router: CoordinatorRouter::new(),
                sockets: HashMap::new(),
                public_pairs: OrderedMap::new(),
                shutting_down: false,
            })),
            routing: Mutex::new(()),
            control_listener: Arc::from(control_listener),
            public_listener: Arc::from(public_listener),
            connector,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            threads: Mutex::new(Vec::new()),
            empty_timer: Arc::new((Mutex::new(None), std::sync::Condvar::new())),
            empty_grace: AtomicU64::new(EMPTY_SHUTDOWN_GRACE_MS),
            process_entry: AtomicBool::new(false),
        });
        server.spawn_control_loop();
        server.spawn_public_loop();
        server
    }

    fn spawn_control_loop(self: &Arc<Self>) {
        let server = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("coordinator-control-accept".to_owned())
            .spawn(move || loop {
                if server.shutdown_flag.load(Ordering::SeqCst) {
                    return;
                }
                let socket = server.control_listener.accept();
                let Ok(socket) = socket else {
                    return;
                };
                if server.shutdown_flag.load(Ordering::SeqCst) {
                    return;
                }
                server.adopt_control_connection(next_connection_id(), socket);
            })
            .expect("spawn coordinator control accept thread");
        self.threads.lock().unwrap().push(thread);
    }

    fn spawn_public_loop(self: &Arc<Self>) {
        let server = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("coordinator-public-accept".to_owned())
            .spawn(move || loop {
                if server.shutdown_flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(client) = server.public_listener.accept() else {
                    return;
                };
                server.adopt_public_connection(client);
            })
            .expect("spawn coordinator public accept thread");
        self.threads.lock().unwrap().push(thread);
    }

    fn adopt_public_connection(self: &Arc<Self>, client: Box<dyn ControlSocket>) {
        let Ok(client) = SharedSocket::new(client) else {
            return;
        };
        let pair_id = next_connection_id();
        let (connection, endpoint) = {
            let _dispatch = self.routing.lock().unwrap();
            let mut state = self.state.lock().unwrap();
            let target = state
                .router
                .current_server
                .as_ref()
                .map(|server| (server.connection.clone(), server.endpoint.clone()));
            let Some(target) = target.filter(|_| !state.shutting_down) else {
                drop(state);
                client.shutdown();
                return;
            };
            state.public_pairs.set(
                pair_id.clone(),
                PublicPair {
                    client: client.clone(),
                    upstream: None,
                },
            );
            state.router.set_public_open(true);
            target
        };
        // Upstream `acceptPublicConnection` cancels the empty timer as soon as
        // a public connection is accepted for proxying.
        self.cancel_empty_shutdown();
        // Node's createConnection is asynchronous. Do not block the accept
        // loop (or routing lock) while a target endpoint connects.
        let server = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("coordinator-public-connect".to_owned())
            .spawn(move || {
                let upstream = server
                    .connector
                    .connect(&endpoint)
                    .and_then(SharedSocket::new);
                let Ok(upstream) = upstream else {
                    server.finish_public_pair(&pair_id);
                    return;
                };
                let accepted = {
                    let _dispatch = server.routing.lock().unwrap();
                    let mut state = server.state.lock().unwrap();
                    let same_server = state
                        .router
                        .current_server
                        .as_ref()
                        .is_some_and(|current| current.connection == connection);
                    if !state.shutting_down && same_server {
                        if let Some(pair) = state.public_pairs.get_mut(&pair_id) {
                            pair.upstream = Some(upstream.clone());
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if accepted {
                    server.spawn_proxy_pair(pair_id, client, upstream);
                } else {
                    upstream.shutdown();
                    server.finish_public_pair(&pair_id);
                }
            })
            .expect("spawn public connection thread");
        self.threads.lock().unwrap().push(thread);
    }

    fn spawn_proxy_pair(
        self: &Arc<Self>,
        pair_id: String,
        client: Arc<SharedSocket>,
        upstream: Arc<SharedSocket>,
    ) {
        for (from, to) in [
            (Arc::clone(&client), Arc::clone(&upstream)),
            (upstream, client),
        ] {
            let server = Arc::clone(self);
            let pair_id = pair_id.clone();
            let thread = std::thread::Builder::new()
                .name("coordinator-public-proxy".to_owned())
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        let read = from.reader().read_bytes(&mut buf);
                        match read {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                if to.as_socket().write_all_bytes(&buf[..read]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    server.finish_public_pair(&pair_id);
                })
                .expect("spawn proxy thread");
            self.threads.lock().unwrap().push(thread);
        }
    }

    fn finish_public_pair(&self, pair_id: &str) {
        let pair = {
            let mut state = self.state.lock().unwrap();
            let pair = state.public_pairs.get(pair_id).cloned();
            state.public_pairs.delete(pair_id);
            let open = !state.public_pairs.is_empty();
            state.router.set_public_open(open);
            pair
        };
        if let Some(pair) = pair {
            pair.shutdown();
        }
        // Upstream's public `finalize()` ends with `checkEmpty()`.
        self.check_empty();
    }

    fn close_public_connections(&self) {
        let pairs = {
            let mut state = self.state.lock().unwrap();
            state.router.set_public_open(false);
            std::mem::take(&mut state.public_pairs)
        };
        for (_, pair) in pairs {
            pair.shutdown();
        }
    }

    fn adopt_control_connection(
        self: &Arc<Self>,
        connection_id: String,
        socket: Box<dyn ControlSocket>,
    ) {
        let Ok(shared) = SharedSocket::new(socket) else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.shutting_down {
                shared.shutdown();
                return;
            }
            state.sockets.insert(connection_id.clone(), shared.clone());
        }
        let server = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("coordinator-control-reader".to_owned())
            .spawn(move || {
                loop {
                    let read = {
                        let mut socket = shared.reader();
                        read_line_capped(socket.as_mut(), "Coordinator message is too large")
                    };
                    let Ok(Some(line)) = read else {
                        let error = read.err().map(|error| error.to_string());
                        server.drop_connection(&connection_id, error);
                        return;
                    };
                    let dispatch = server.routing.lock().unwrap();
                    let outcome = {
                        let mut state = server.state.lock().unwrap();
                        if state.shutting_down {
                            return;
                        }
                        state.router.handle_line(&connection_id, &line)
                    };
                    match outcome {
                        Ok(actions) => {
                            // Upstream `registerServer`/`registerPeer` call
                            // `cancelEmptyShutdown()` before writing the reply.
                            let registered_role = actions.iter().any(|action| {
                                matches!(action, RouterAction::Send { message, .. } if {
                                    let kind = message["type"].as_str().unwrap_or_default();
                                    kind == "server_registered" || kind == "peer_registered"
                                })
                            });
                            if registered_role {
                                server.cancel_empty_shutdown();
                            }
                            server.execute(actions);
                        }
                        Err(error) => {
                            drop(dispatch);
                            server.destroy_connection(&connection_id, error);
                        }
                    }
                }
            })
            .expect("spawn control reader thread");
        self.threads.lock().unwrap().push(thread);
    }

    fn execute(self: &Arc<Self>, actions: Vec<RouterAction>) {
        for action in actions {
            match action {
                RouterAction::Send {
                    connection,
                    message,
                } => {
                    let Ok(line) = encode_control_line(&message) else {
                        continue;
                    };
                    let socket = self.state.lock().unwrap().sockets.get(&connection).cloned();
                    if let Some(socket) = socket {
                        // Never hold global state while a peer applies backpressure:
                        // shutdown must be able to close this socket out of band.
                        let _ = socket.as_socket().write_line(&line);
                    }
                }
                RouterAction::ClosePublic => self.close_public_connections(),
            }
        }
    }

    fn drop_connection(self: &Arc<Self>, connection: &str, destroy_error: Option<String>) {
        let _ = destroy_error;
        let _dispatch = self.routing.lock().unwrap();
        let (socket, actions) = {
            let mut state = self.state.lock().unwrap();
            let socket = state.sockets.remove(connection);
            let actions = if state.shutting_down {
                Vec::new()
            } else {
                state.router.handle_disconnect(connection)
            };
            (socket, actions)
        };
        if let Some(socket) = socket {
            socket.shutdown();
        }
        self.execute(actions);
        // Upstream's disconnect handler ends with `checkEmpty()`.
        self.check_empty();
    }

    fn destroy_connection(self: &Arc<Self>, connection: &str, error: String) {
        self.drop_connection(connection, Some(error));
    }

    /// Upstream `checkEmpty` probe: true when no server, peer, control or
    /// public connection is live.
    pub fn is_idle(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.router.is_empty() && state.sockets.is_empty()
    }

    // ── upstream empty-shutdown timer (`scheduleEmptyShutdown` family) ─────

    /// Upstream `scheduleEmptyShutdown(delayMs)`: arms the single timer slot;
    /// a no-op when a timer is already armed or the server is shutting down.
    fn schedule_empty_shutdown(&self, delay: Duration) {
        let (lock, signal) = &*self.empty_timer;
        let mut deadline = lock.lock().unwrap();
        if deadline.is_some() || self.shutdown_flag.load(Ordering::SeqCst) {
            return;
        }
        *deadline = Some(Instant::now() + delay);
        signal.notify_all();
    }

    /// Upstream `cancelEmptyShutdown()`.
    fn cancel_empty_shutdown(&self) {
        let (lock, signal) = &*self.empty_timer;
        *lock.lock().unwrap() = None;
        signal.notify_all();
    }

    /// Upstream `checkEmpty()`: non-empty cancels the timer; empty re-arms it
    /// with `EMPTY_SHUTDOWN_GRACE_MS` (unless one is already armed).
    fn check_empty(&self) {
        if self.is_idle() {
            let grace = self.empty_grace.load(Ordering::SeqCst);
            self.schedule_empty_shutdown(Duration::from_millis(grace));
        } else {
            self.cancel_empty_shutdown();
        }
    }

    /// Upstream `main()`'s trailing `scheduleEmptyShutdown(EMPTY_STARTUP_GRACE_MS)`
    /// plus the timer callback as a watchdog thread: when the armed deadline
    /// fires and the process is still empty, [`CoordinatorServer::shutdown`]
    /// runs (upstream `process.exit(0)`).
    pub fn start_empty_shutdown_watchdog(
        self: &Arc<Self>,
        startup_grace: Duration,
        empty_grace: Duration,
    ) {
        self.empty_grace
            .store(empty_grace.as_millis() as u64, Ordering::SeqCst);
        self.schedule_empty_shutdown(startup_grace);
        let server = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("coordinator-empty-shutdown".to_owned())
            .spawn(move || {
                let (lock, signal) = &*server.empty_timer;
                let mut deadline = lock.lock().unwrap();
                loop {
                    if server.shutdown_flag.load(Ordering::SeqCst) {
                        return;
                    }
                    let Some(at) = *deadline else {
                        deadline = signal.wait(deadline).unwrap();
                        continue;
                    };
                    let now = Instant::now();
                    if now < at {
                        let (guard, _) = signal.wait_timeout(deadline, at - now).unwrap();
                        deadline = guard;
                        if *deadline == Some(at) && Instant::now() < at {
                            continue;
                        }
                    }
                    if *deadline != Some(at) {
                        continue;
                    }
                    *deadline = None;
                    drop(deadline);
                    if server.is_idle() {
                        server.shutdown();
                        return;
                    }
                    deadline = lock.lock().unwrap();
                }
            })
            .expect("spawn coordinator empty-shutdown watchdog");
        self.threads.lock().unwrap().push(thread);
    }

    #[cfg(test)]
    pub(super) fn connection_counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.sockets.len(), state.public_pairs.len())
    }

    /// Upstream `shutdownCoordinator`.
    pub fn shutdown(&self) {
        if self.shutdown_flag.swap(true, Ordering::SeqCst) {
            return;
        }
        // Upstream ends with `process.exit(0)`: the coordinator process is
        // gone, so a fresh process starts with `running === false`. The
        // in-process face clears the `run_coordinator_process` entry flag.
        if self.process_entry.load(Ordering::SeqCst) {
            COORDINATOR_RUNNING.store(false, Ordering::SeqCst);
        }
        self.cancel_empty_shutdown();
        let (sockets, pairs) = {
            let mut state = self.state.lock().unwrap();
            state.shutting_down = true;
            state.router = CoordinatorRouter::new();
            (
                std::mem::take(&mut state.sockets),
                std::mem::take(&mut state.public_pairs),
            )
        };
        for (_, pair) in pairs {
            pair.shutdown();
        }
        for socket in sockets.into_values() {
            socket.shutdown();
        }
        self.control_listener.close();
        self.public_listener.close();
    }

    /// Blocks until the server goes idle (used by tests and by an embedder's
    /// empty-shutdown watchdog: `scheduleEmptyShutdown(EMPTY_STARTUP_GRACE_MS)`
    /// then `EMPTY_SHUTDOWN_GRACE_MS` once idle, per upstream `checkEmpty`).
    pub fn wait_idle(&self, poll: Duration) -> bool {
        loop {
            if self.shutdown_flag.load(Ordering::SeqCst) {
                return false;
            }
            if self.is_idle() {
                return true;
            }
            std::thread::sleep(poll);
        }
    }
}

struct SharedSocketState {
    reader: Mutex<Box<dyn ControlSocket>>,
    socket: Mutex<Box<dyn ControlSocket>>,
    closer: Mutex<Box<dyn ControlSocket>>,
}

/// Cloneable handle over one socket so the reader thread and out-of-band
/// writers share the same connection.
#[derive(Clone)]
struct SharedSocket {
    state: Arc<SharedSocketState>,
}

impl SharedSocket {
    fn new(socket: Box<dyn ControlSocket>) -> std::io::Result<Arc<Self>> {
        let reader = socket.try_clone().ok_or_else(|| {
            std::io::Error::other("Coordinator socket cannot split read/write handles")
        })?;
        let closer = socket.try_clone().ok_or_else(|| {
            std::io::Error::other("Coordinator socket cannot split shutdown handle")
        })?;
        Ok(Arc::new(SharedSocket {
            state: Arc::new(SharedSocketState {
                reader: Mutex::new(reader),
                socket: Mutex::new(socket),
                closer: Mutex::new(closer),
            }),
        }))
    }
    fn reader(&self) -> std::sync::MutexGuard<'_, Box<dyn ControlSocket>> {
        self.state.reader.lock().unwrap()
    }

    fn as_socket(&self) -> std::sync::MutexGuard<'_, Box<dyn ControlSocket>> {
        self.state.socket.lock().unwrap()
    }

    fn shutdown(&self) {
        self.state.closer.lock().unwrap().shutdown();
    }
}

fn next_connection_id() -> String {
    static SERIAL: AtomicU64 = AtomicU64::new(1);
    format!("control#{}", SERIAL.fetch_add(1, Ordering::SeqCst))
}

// ── Client-side connection ─────────────────────────────────────────────────

/// Upstream `CoordinatorConnection`: the server-side endpoint of the
/// coordinator's intentionally opaque message router, over the transport
/// seam. Events are delivered through the channel from
/// [`CoordinatorConnection::event_channel`] (upstream: listener callbacks);
/// `replaced()` resolves when the coordinator replaces this server or the
/// connection drops.
pub struct CoordinatorConnection {
    pub server_connection_id: String,
    control_path: String,
    endpoint: String,
    inner: Arc<ConnectionShared>,
}

struct ConnectionShared {
    events: Mutex<Option<std::sync::mpsc::Sender<CoordinatorConnectionEvent>>>,
    replaced: tokio::sync::Notify,
    was_replaced: AtomicBool,
    registered: Mutex<Option<tokio::sync::oneshot::Sender<Result<(), String>>>>,
    registered_done: AtomicBool,
    peer_ids: Mutex<OrderedMap<()>>,
    expected_server_connection_id: Mutex<String>,
    socket: Mutex<Option<Arc<SharedSocket>>>,
    closed: AtomicBool,
}

impl CoordinatorConnection {
    pub fn new(
        control_path: impl Into<String>,
        endpoint: impl Into<String>,
        server_connection_id: Option<String>,
    ) -> Self {
        let server_connection_id = server_connection_id.unwrap_or_else(crate::ai::uuid::uuid_v7);
        CoordinatorConnection {
            server_connection_id: server_connection_id.clone(),
            control_path: control_path.into(),
            endpoint: endpoint.into(),
            inner: Arc::new(ConnectionShared {
                events: Mutex::new(None),
                replaced: tokio::sync::Notify::new(),
                was_replaced: AtomicBool::new(false),
                registered: Mutex::new(None),
                registered_done: AtomicBool::new(false),
                peer_ids: Mutex::new(OrderedMap::new()),
                expected_server_connection_id: Mutex::new(server_connection_id.clone()),
                socket: Mutex::new(None),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Installs the event receiver; must be called before [`Self::connect`]
    /// so no early event is lost (upstream `onEvent` before `connect`).
    pub fn event_channel(&self) -> std::sync::mpsc::Receiver<CoordinatorConnectionEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.inner.events.lock().unwrap() = Some(tx);
        rx
    }

    pub fn control_path(&self) -> &str {
        &self.control_path
    }

    pub fn was_replaced(&self) -> bool {
        self.inner.was_replaced.load(Ordering::SeqCst)
    }

    /// Upstream `replaced` promise.
    pub async fn replaced(&self) {
        loop {
            if self.was_replaced() {
                return;
            }
            self.inner.replaced.notified().await;
        }
    }

    pub fn peer_ids(&self) -> Vec<String> {
        self.inner
            .peer_ids
            .lock()
            .unwrap()
            .keys()
            .map(str::to_owned)
            .collect()
    }

    /// Upstream `connect()`: connect the socket, send the `register_server`
    /// frame, resolve once `server_registered` confirms our id.
    pub async fn connect(&self, connector: &dyn ControlConnector) -> Result<(), String> {
        if self.inner.socket.lock().unwrap().is_some() {
            return Err("Coordinator server is already connected".to_string());
        }
        let socket = SharedSocket::new(
            connector
                .connect(&self.control_path)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
        *self.inner.registered.lock().unwrap() = Some(registered_tx);
        let line = encode_control_line(&json!({
            "type": "register_server",
            "protocol": COORDINATOR_PROTOCOL_VERSION,
            "serverConnectionId": self.server_connection_id,
            "endpoint": self.endpoint,
        }))
        .map_err(|error| error.to_string())?;
        if let Err(error) = socket.as_socket().write_line(&line) {
            *self.inner.registered.lock().unwrap() = None;
            return Err(error.to_string());
        }
        *self.inner.socket.lock().unwrap() = Some(socket);
        self.spawn_reader();
        match registered_rx.await {
            Ok(result) => result,
            Err(_) => Err("Coordinator connection closed".to_string()),
        }
    }

    fn spawn_reader(&self) {
        let socket = self
            .inner
            .socket
            .lock()
            .unwrap()
            .clone()
            .expect("connected");
        let shared = Arc::clone(&self.inner);
        let thread = std::thread::Builder::new()
            .name("coordinator-connection-reader".to_owned())
            .spawn(move || loop {
                let read = {
                    let mut guard = socket.reader();
                    read_line_capped(guard.as_mut(), "Coordinator message is too large")
                };
                match read {
                    Ok(Some(line)) => {
                        if !shared.apply_line(&line) {
                            return;
                        }
                    }
                    Ok(None) => {
                        shared.disconnected("Coordinator connection closed");
                        return;
                    }
                    Err(error) => {
                        shared.disconnected(&error.to_string());
                        return;
                    }
                }
            })
            .expect("spawn connection reader thread");
        let _ = thread;
    }

    /// Upstream `send(peerId, payload)`.
    pub async fn send(&self, peer_id: &str, payload: Value) -> Result<(), String> {
        self.write(json!({ "type": "send", "to": peer_id, "payload": payload }))
            .await
    }

    /// Upstream `broadcast(payload)`.
    pub async fn broadcast(&self, payload: Value) -> Result<(), String> {
        self.write(json!({ "type": "broadcast", "payload": payload }))
            .await
    }

    /// Upstream `#write`: rejects before registration or after close.
    pub async fn write(&self, message: Value) -> Result<(), String> {
        if !self.is_registered() || self.inner.closed.load(Ordering::SeqCst) {
            return Err("Coordinator server is not connected".to_string());
        }
        let line = encode_control_line(&message).map_err(|error| error.to_string())?;
        let socket = self.inner.socket.lock().unwrap().clone();
        let Some(socket) = socket else {
            return Err("Coordinator server is not connected".to_string());
        };
        let write = socket.as_socket().write_line(&line);
        write.map_err(|error| error.to_string())
    }

    fn is_registered(&self) -> bool {
        self.inner.registered.lock().unwrap().is_none()
            && self.inner.registered_done.load(Ordering::SeqCst)
    }

    /// Upstream `close()`: destroys the socket, clears the peer set and
    /// rejects a pending registration. Upstream deliberately does NOT resolve
    /// the `replaced` promise here (`#disconnected` early-returns once
    /// `#closed` is set), so neither does the port.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.peer_ids.lock().unwrap().clear();
        if let Some(socket) = self.inner.socket.lock().unwrap().take() {
            socket.shutdown();
        }
        if let Some(tx) = self.inner.registered.lock().unwrap().take() {
            let _ = tx.send(Err("Coordinator server closed".to_string()));
        }
    }
}

impl ConnectionShared {
    fn expected_server_connection_id(&self) -> String {
        self.expected_server_connection_id.lock().unwrap().clone()
    }

    /// Upstream `#disconnected`: a socket close/error after an explicit
    /// `close()` is swallowed (`if (this.#closed) return`) — no registration
    /// rejection and no `replaced` resolution.
    fn destroy(&self, registered_error: &str) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        if let Some(socket) = self.socket.lock().unwrap().take() {
            socket.shutdown();
        }
        if let Some(tx) = self.registered.lock().unwrap().take() {
            let _ = tx.send(Err(registered_error.to_string()));
        }
        self.mark_replaced();
    }

    fn disconnected(&self, error: &str) {
        self.destroy(error);
    }

    fn mark_replaced(&self) {
        if !self.was_replaced.swap(true, Ordering::SeqCst) {
            self.replaced.notify_waiters();
        }
    }

    /// Upstream `#handleMessage`; returns false when the socket was
    /// destroyed.
    fn apply_line(&self, line: &str) -> bool {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            self.destroy("Coordinator sent invalid JSON");
            return false;
        };
        let Ok(message) = serde_json::from_value::<CoordinatorMessage>(message) else {
            self.destroy("Coordinator sent an invalid message");
            return false;
        };
        match message {
            CoordinatorMessage::ServerRegistered {
                server_connection_id,
                peers,
            } => {
                if server_connection_id != self.expected_server_connection_id() {
                    self.destroy("Coordinator returned an invalid server registration");
                    return false;
                }
                {
                    let mut peer_ids = self.peer_ids.lock().unwrap();
                    for peer in peers {
                        peer_ids.set(peer, ());
                    }
                }
                self.registered_done.store(true, Ordering::SeqCst);
                if let Some(tx) = self.registered.lock().unwrap().take() {
                    let _ = tx.send(Ok(()));
                }
                true
            }
            CoordinatorMessage::ServerReplaced => {
                self.mark_replaced();
                true
            }
            CoordinatorMessage::PeerConnected { peer_id } => {
                self.peer_ids.lock().unwrap().set(peer_id.clone(), ());
                self.emit(CoordinatorConnectionEvent::PeerConnected { peer_id });
                true
            }
            CoordinatorMessage::PeerDisconnected { peer_id } => {
                self.peer_ids.lock().unwrap().delete(&peer_id);
                self.emit(CoordinatorConnectionEvent::PeerDisconnected { peer_id });
                true
            }
            CoordinatorMessage::Message { from, payload } => {
                self.emit(CoordinatorConnectionEvent::Message { from, payload });
                true
            }
        }
    }

    fn emit(&self, event: CoordinatorConnectionEvent) {
        if let Some(events) = self.events.lock().unwrap().as_ref() {
            let _ = events.send(event);
        }
    }
}

// ── Startup lease ──────────────────────────────────────────────────────────

/// Upstream `CoordinatorStartupLease`.
pub struct CoordinatorStartupLease {
    socket: Box<dyn ControlSocket>,
}

impl std::fmt::Debug for CoordinatorStartupLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CoordinatorStartupLease")
    }
}

impl CoordinatorStartupLease {
    pub fn close(mut self) {
        self.socket.shutdown();
    }
}

/// Upstream `tryConnect`: connect, mapping ENOENT/ECONNREFUSED to `None`.
pub(crate) fn try_connect(
    connector: &dyn ControlConnector,
    path: &str,
) -> Result<Option<Box<dyn ControlSocket>>, String> {
    match connector.connect(path) {
        Ok(socket) => Ok(Some(socket)),
        Err(error) => match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => Ok(None),
            _ => Err(error.to_string()),
        },
    }
}

/// Upstream `ensureCoordinator`: reuse a live coordinator or spawn one and
/// poll for startup.
pub async fn ensure_coordinator(
    public_path: &str,
    control_path: &str,
    spawner: &dyn ProcessSpawner,
    connector: &dyn ControlConnector,
) -> Result<CoordinatorStartupLease, String> {
    if let Some(existing) = try_connect(connector, control_path)? {
        return Ok(CoordinatorStartupLease { socket: existing });
    }
    let child: Box<dyn InternalProcessChild> = spawner
        .spawn(
            InternalProcessRole::Coordinator,
            &[public_path.to_owned(), control_path.to_owned()],
            &[],
        )
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(COORDINATOR_START_TIMEOUT_MS);
    loop {
        if let Some(socket) = try_connect(connector, control_path)? {
            return Ok(CoordinatorStartupLease { socket });
        }
        if child.has_exited() {
            return Err("Coordinator exited during startup".to_string());
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for coordinator startup".to_string());
        }
        tokio::time::sleep(Duration::from_millis(COORDINATOR_RETRY_MS)).await;
    }
}

// ── Socket-path hygiene and the coordinator process entry ──────────────────

/// Upstream `restrictSocket`: `chmod 0o600` on non-Windows platforms; the
/// win32 named-pipe face upstream is a no-op, and so is the loopback-TCP
/// transport on Windows here.
pub fn restrict_socket(path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Upstream `cleanupSocket`: unlink the socket path, tolerating ENOENT. On
/// Windows upstream returns early (`process.platform === "win32"`), and the
/// loopback-TCP transport has no filesystem state to clean.
pub fn cleanup_socket(path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Upstream `removeStaleSocket`: an existing path must be a socket (else
/// `"Coordinator path is not a socket: {path}"`), must not accept connections
/// (else `"Coordinator socket is already active: {path}"`), and is otherwise
/// unlinked. A missing path is fine; other lstat/connect errors propagate.
/// On Windows upstream's `lstat` on a named pipe fails with ENOENT and the
/// call returns, so the non-unix face is a no-op here as well.
pub fn remove_stale_socket(path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.file_type().is_socket() {
            return Err(format!("Coordinator path is not a socket: {path}"));
        }
        let live = match std::os::unix::net::UnixStream::connect(path) {
            Ok(stream) => {
                drop(stream);
                true
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionRefused
                    || error.kind() == std::io::ErrorKind::NotFound =>
            {
                false
            }
            Err(error) => return Err(error.to_string()),
        };
        if live {
            return Err(format!("Coordinator socket is already active: {path}"));
        }
        cleanup_socket(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

static COORDINATOR_RUNNING: AtomicBool = AtomicBool::new(false);

/// Upstream `runCoordinatorProcess`: the embeddable coordinator entrypoint.
/// Validates the argument pair, cleans stale socket paths, listens on the
/// control then the public endpoint (restricting permissions after each bind
/// like upstream `main()`), and arms the upstream empty-shutdown timer with
/// the fixed `EMPTY_STARTUP_GRACE_MS` / `EMPTY_SHUTDOWN_GRACE_MS` constants.
///
/// Platform disclosures: upstream installs SIGINT/SIGTERM handlers that call
/// `shutdownCoordinator`; `std` has no portable signal registration, so the
/// embedder owns signals and calls [`CoordinatorServer::shutdown`]. Upstream
/// `process.exit(0)` becomes this function returning the running
/// [`CoordinatorServer`]; the caller keeps it alive, and shutting that server
/// down clears the `running` flag (a fresh coordinator process starts with
/// `running === false`). A successful call marks this process as running
/// (`"Coordinator process is already running"` on re-entry), mirroring
/// upstream's `running` flag.
pub fn run_coordinator_process(args: &[String]) -> Result<Arc<CoordinatorServer>, String> {
    if COORDINATOR_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Coordinator process is already running".to_string());
    }
    let outcome = run_coordinator_process_inner(args);
    if outcome.is_err() {
        COORDINATOR_RUNNING.store(false, Ordering::SeqCst);
    }
    outcome
}

fn run_coordinator_process_inner(args: &[String]) -> Result<Arc<CoordinatorServer>, String> {
    let (public_path, control_path) = match args {
        [public_path, control_path] => (public_path, control_path),
        _ => return Err("Coordinator requires public and control socket paths".to_string()),
    };
    remove_stale_socket(control_path)?;
    remove_stale_socket(public_path)?;
    let control_listener =
        match crate::coding_agent::experimental::coordinator::transport::bind_platform_listener(
            control_path,
        ) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = cleanup_socket(control_path);
                let _ = cleanup_socket(public_path);
                return Err(error.to_string());
            }
        };
    if let Err(error) = restrict_socket(control_path) {
        let _ = cleanup_socket(control_path);
        let _ = cleanup_socket(public_path);
        return Err(error);
    }
    let public_listener =
        match crate::coding_agent::experimental::coordinator::transport::bind_platform_listener(
            public_path,
        ) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = cleanup_socket(control_path);
                let _ = cleanup_socket(public_path);
                return Err(error.to_string());
            }
        };
    if let Err(error) = restrict_socket(public_path) {
        let _ = cleanup_socket(control_path);
        let _ = cleanup_socket(public_path);
        return Err(error);
    }
    let server = CoordinatorServer::bind(
        control_listener,
        public_listener,
        Arc::from(crate::coding_agent::experimental::coordinator::transport::platform_connector()),
    );
    server.process_entry.store(true, Ordering::SeqCst);
    CoordinatorServer::start_empty_shutdown_watchdog(
        &server,
        Duration::from_millis(EMPTY_STARTUP_GRACE_MS),
        Duration::from_millis(EMPTY_SHUTDOWN_GRACE_MS),
    );
    Ok(server)
}
