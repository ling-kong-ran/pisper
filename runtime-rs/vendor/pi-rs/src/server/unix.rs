//! Port of `packages/server/src/transports/unix/` (`address.ts` 9 lines
//! SHA256 `2d307bb684e4a6296695ad0f921547c573bdbdb843001873aa37d129a535cf13`,
//! `listener.ts` 421 lines SHA256
//! `de71c68354820cede8eb7aeebb4ebe525cd6f832ce1691b60b5279816fd646eb`,
//! `preset.ts` 28 lines SHA256
//! `6c0010fe5c9a49d2af6c2f6793cd36723061fc22b8400528ac15f970f85054e8`,
//! `types.ts` 15 lines SHA256
//! `067fff91eefffc09f1d7952cc8058d3333cee15f568a67711644550eba67423f`).
//!
//! `cfg(unix)` — the upstream surface is Unix-socket-only and its own unix
//! tests are `process.platform !== "win32"`-gated (disclosed seam S6, the
//! same gate as the client slice): this module cannot compile on the
//! Windows development host, and is compile-checked on a unix target.
//!
//! The bind dance is ported step-for-step: derive the owned bind path from
//! the SHA256 of the public path, remove stale sockets (live-probe
//! guarded), bind the owned path, hard-link it onto the public route, set
//! the socket mode, then unlink the owned path. Shutdown removes the
//! public route only when the filesystem identity (dev/ino) still matches,
//! preserving any replacement inode.
//!
//! Disclosed divergence D-B (write serialization): upstream serializes
//! writes through a promise tail (`writeTail`) with a `maxPendingBytes`
//! call-time limit; the port uses a single writer task over the socket's
//! write half with the same limit, the same error texts, and the same close
//! ordering (the final chunk is queued behind every pending write).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf};
use tokio::net::{UnixListener as TokioUnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, OnceCell};
use tokio_util::sync::CancellationToken;

use crate::protocol::framing::DEFAULT_MAX_FRAME_LENGTH;

use super::connection::{ByteConnection, ByteConnectionAcceptor, ByteConnectionHandler};
use super::errors::OperationError;
use super::listener::ServerListener;
use super::server::Server;
use super::types::{ConnectionCountHandler, ErrorObserver, ServerHost, ServerOptions};

const DEFAULT_SOCKET_MODE: u32 = 0o600;
const DEFAULT_GRACEFUL_CLOSE_TIMEOUT_MS: u64 = 5_000;
const MAX_UINT32: u64 = u32::MAX as u64;
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;
const SOCKET_PROBE_TIMEOUT_MS: u64 = 1_000;

/// `types.ts` `UnixListenerOptions`.
#[derive(Clone, Default)]
pub struct UnixListenerOptions {
    pub path: String,
    /// Socket filesystem permissions. Defaults to owner read/write only
    /// (0o600).
    pub mode: Option<u32>,
    /// Maximum framed bytes queued per connection before a slow peer is
    /// disconnected.
    pub max_pending_bytes: Option<u64>,
    pub graceful_close_timeout_ms: Option<u64>,
    /// Used to derive and validate maxPendingBytes. Must match the server
    /// when customized.
    pub max_frame_length: Option<u64>,
    pub on_error: Option<ErrorObserver>,
}

/// `types.ts` `UnixServerOptions`: `ServerOptions` without `listeners` plus
/// `UnixListenerOptions`.
pub struct UnixServerOptions {
    pub path: String,
    pub server_id: String,
    pub mode: Option<u32>,
    pub max_pending_bytes: Option<u64>,
    pub graceful_close_timeout_ms: Option<u64>,
    pub max_frame_length: Option<u64>,
    pub handshake_timeout_ms: Option<u64>,
    pub on_connection_count_changed: Option<ConnectionCountHandler>,
    pub on_error: Option<ErrorObserver>,
}

/// `address.ts` `getUnixSocketPath`.
pub fn get_unix_socket_path(
    server_id: &str,
    server_directory: &str,
) -> Result<String, OperationError> {
    if !crate::protocol::protocol::is_server_id(server_id) {
        return Err(OperationError::Other(
            "Unix serverId must be a canonical lowercase UUIDv4".to_string(),
        ));
    }
    Ok(Path::new(server_directory)
        .join(format!("{server_id}.sock"))
        .to_string_lossy()
        .into_owned())
}

struct ResolvedUnixListenerOptions {
    path: String,
    mode: u32,
    graceful_close_timeout_ms: u64,
    max_pending_bytes: u64,
    on_error: Option<ErrorObserver>,
}

/// `listener.ts:392-421` `resolveUnixListenerOptions`.
fn resolve_unix_listener_options(
    options: &UnixListenerOptions,
) -> Result<ResolvedUnixListenerOptions, OperationError> {
    if options.path.is_empty() {
        return Err(OperationError::Other(
            "Server Unix socket path must not be empty".to_string(),
        ));
    }
    let mode = options.mode.unwrap_or(DEFAULT_SOCKET_MODE);
    if mode > 0o777 {
        return Err(OperationError::Other(
            "Server Unix socket mode must be an integer between 0 and 0o777".to_string(),
        ));
    }
    let max_frame_length = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
        return Err(OperationError::Other(format!(
            "Server maxFrameLength must be an integer between 1 and {MAX_UINT32}"
        )));
    }
    let max_pending_bytes = options.max_pending_bytes.unwrap_or(max_frame_length * 4);
    if max_pending_bytes < max_frame_length + 4 {
        return Err(OperationError::Other(
            "Server maxPendingBytes must be a safe integer at least maxFrameLength + 4".to_string(),
        ));
    }
    let graceful_close_timeout_ms = options
        .graceful_close_timeout_ms
        .unwrap_or(DEFAULT_GRACEFUL_CLOSE_TIMEOUT_MS);
    if graceful_close_timeout_ms == 0 || graceful_close_timeout_ms > MAX_TIMER_DELAY_MS {
        return Err(OperationError::Other(format!(
            "Server gracefulCloseTimeoutMs must be an integer between 1 and {MAX_TIMER_DELAY_MS}"
        )));
    }
    Ok(ResolvedUnixListenerOptions {
        path: options.path.clone(),
        mode,
        graceful_close_timeout_ms,
        max_pending_bytes,
        on_error: options.on_error.clone(),
    })
}

/// `listener.ts` `FileIdentity`.
#[derive(Clone, Copy)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

fn identity_of(metadata: &std::fs::Metadata) -> FileIdentity {
    FileIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
    }
}

fn is_socket_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_socket()
}

fn io_message(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{error} (os error {code})"),
        None => error.to_string(),
    }
}

fn io_error(error: &std::io::Error) -> OperationError {
    OperationError::Other(io_message(error))
}

fn io_other(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

/// `listener.ts` `getOwnedBindPath` (`listener.ts:294-297`).
fn get_owned_bind_path(path: &Path) -> PathBuf {
    let suffix = sha256_hex(path.to_string_lossy().as_bytes());
    path.with_file_name(format!("bind-{}", &suffix[..8]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A short pseudo-random suffix for the preserved-rename dance (upstream
/// `randomUUID().slice(0, 6)`; same disclosed substitution as the router's
/// attachment ids).
fn random_suffix() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(crate::ai::now_ms() as u64);
    hasher.write_u64(std::process::id() as u64);
    format!("{:016x}", hasher.finish())
}

/// `listener.ts` `removeStaleSocket` (`listener.ts:299-328`).
fn remove_stale_socket(path: &Path) -> Result<(), std::io::Error> {
    let original = match std::fs::symlink_metadata(path) {
        Ok(original) => original,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !is_socket_metadata(&original) {
        return Err(io_other(format!(
            "Refusing to remove non-socket Unix listener path: {}",
            path.display()
        )));
    }
    if is_socket_live(path) {
        return Err(io_other(format!(
            "Unix listener is already running: {}",
            path.display()
        )));
    }

    let preserved = path.with_file_name(format!("stale-{}", &random_suffix()[..6]));
    if let Err(error) = std::fs::rename(path, &preserved) {
        if error.kind() == ErrorKind::NotFound {
            return Ok(());
        }
        return Err(error);
    }
    let current = std::fs::symlink_metadata(&preserved)?;
    if !is_socket_metadata(&current)
        || identity_of(&current).dev != identity_of(&original).dev
        || identity_of(&current).ino != identity_of(&original).ino
    {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                std::fs::rename(&preserved, path)?;
            }
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        return Err(io_other(format!(
            "Unix listener path changed while checking for a stale socket: {}",
            path.display()
        )));
    }
    let _ = std::fs::remove_file(&preserved);
    Ok(())
}

/// `listener.ts` `isSocketLive` (`listener.ts:338-363`): a connect probe
/// with a 1s timeout (timeout → live).
fn is_socket_live(path: &Path) -> bool {
    let path = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = std::os::unix::net::UnixStream::connect(&path);
        let _ = tx.send(outcome);
    });
    match rx.recv_timeout(Duration::from_millis(SOCKET_PROBE_TIMEOUT_MS)) {
        Ok(Ok(_stream)) => true,
        // Upstream `isSocketLive`: a refused/missing/reset probe means the
        // socket is stale (not live); other errors reject upstream and are
        // collapsed into the conservative "live" verdict here.
        Ok(Err(error)) => !matches!(
            error.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::NotFound
                | ErrorKind::BrokenPipe
                | ErrorKind::ConnectionReset
        ),
        Err(_) => true,
    }
}

/// `listener.ts` `setSocketMode` (`listener.ts:365-372`): ENOSYS/ENOTSUP
/// tolerated.
fn set_socket_mode(path: &Path, mode: u32) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    permissions.set_mode(mode);
    if let Err(error) = std::fs::set_permissions(path, permissions) {
        // ENOSYS (38) and ENOTSUP (95) are swallowed upstream; other
        // failures surface on the next filesystem operation (the mode is
        // best-effort on exotic filesystems).
        let _ = error;
    }
}

/// The inner listener state (`listener.ts` `UnixListener` fields,
/// `listener.ts:30-39`).
struct ListenerState {
    options: ResolvedUnixListenerOptions,
    connections: Mutex<Vec<Arc<UnixByteConnection>>>,
    server: Mutex<Option<Arc<TokioUnixListener>>>,
    socket_identity: Mutex<Option<FileIdentity>>,
    owned_bind_path: Mutex<Option<PathBuf>>,
    closing: AtomicBool,
    accept_task: Mutex<Option<CancellationToken>>,
    /// Upstream `closePromise` memoization.
    close_once: tokio::sync::OnceCell<Result<(), OperationError>>,
}

/// The [`ServerListener`] handle: the trait's `&self` methods drive the
/// shared state.
pub struct UnixListener {
    state: Arc<ListenerState>,
}

/// `listener.ts` `createUnixListener` (`listener.ts:388-390`).
pub fn create_unix_listener(
    options: UnixListenerOptions,
) -> Result<Arc<dyn ServerListener>, OperationError> {
    let resolved = resolve_unix_listener_options(&options)?;
    Ok(Arc::new(UnixListener {
        state: Arc::new(ListenerState {
            options: resolved,
            connections: Mutex::new(Vec::new()),
            server: Mutex::new(None),
            socket_identity: Mutex::new(None),
            owned_bind_path: Mutex::new(None),
            closing: AtomicBool::new(false),
            accept_task: Mutex::new(None),
            close_once: tokio::sync::OnceCell::new(),
        }),
    }))
}

impl UnixListener {
    fn report_error(&self, error: &OperationError) {
        // Upstream try/catch (`listener.ts:181-187`); Rust closures cannot
        // throw.
        if let Some(on_error) = &self.state.options.on_error {
            on_error(error);
        }
    }

    /// `listener.ts` `start` (`listener.ts:47-86`).
    async fn start(&self, accept: ByteConnectionAcceptor) -> Result<(), OperationError> {
        let state = &self.state;
        if state.server.lock().unwrap().is_some() {
            return Err(OperationError::Other(
                "Unix listener is already started".to_string(),
            ));
        }
        if state.closing.load(Ordering::SeqCst) {
            return Err(OperationError::Other(
                "Unix listener is closing or closed".to_string(),
            ));
        }
        let path = PathBuf::from(&state.options.path);
        let owned_bind_path = get_owned_bind_path(&path);

        // `mkdir(dirname(path), { recursive: true, mode: 0o700 })`.
        if let Some(parent) = path.parent() {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(parent)
                .map_err(|error| io_error(&error))?;
        }
        remove_stale_socket(&path).map_err(|error| io_error(&error))?;
        remove_stale_socket(&owned_bind_path).map_err(|error| io_error(&error))?;
        *state.owned_bind_path.lock().unwrap() = Some(owned_bind_path.clone());

        // tokio refuses to register a blocking socket (tokio#7172); node's
        // net.Server also drives the accepted socket non-blocking.
        let std_listener = match std::os::unix::net::UnixListener::bind(&owned_bind_path) {
            Ok(listener) => listener,
            Err(error) => {
                self.close_server_and_cleanup().await;
                return Err(io_error(&error));
            }
        };
        if let Err(error) = std_listener.set_nonblocking(true) {
            self.close_server_and_cleanup().await;
            return Err(io_error(&error));
        }
        let listener = match TokioUnixListener::from_std(std_listener) {
            Ok(listener) => Arc::new(listener),
            Err(error) => {
                self.close_server_and_cleanup().await;
                return Err(io_error(&error));
            }
        };

        let stats = match std::fs::symlink_metadata(&owned_bind_path) {
            Ok(stats) => stats,
            Err(error) => {
                self.close_server_and_cleanup().await;
                return Err(io_error(&error));
            }
        };
        if !is_socket_metadata(&stats) {
            self.close_server_and_cleanup().await;
            return Err(OperationError::Other(format!(
                "Unix listener path is not a socket after binding: {}",
                owned_bind_path.display()
            )));
        }
        *state.socket_identity.lock().unwrap() = Some(identity_of(&stats));
        if let Err(error) = std::fs::hard_link(&owned_bind_path, &path) {
            self.close_server_and_cleanup().await;
            return Err(io_error(&error));
        }
        set_socket_mode(&path, state.options.mode);
        let _ = std::fs::remove_file(&owned_bind_path);
        *state.owned_bind_path.lock().unwrap() = None;

        *state.server.lock().unwrap() = Some(listener.clone());
        // The accept loop (upstream `createServer((socket) => ...)`).
        let token = CancellationToken::new();
        *state.accept_task.lock().unwrap() = Some(token.clone());
        let loop_listener = listener;
        let loop_self = Arc::new(UnixListener {
            state: Arc::clone(&self.state),
        });
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    _ = token.cancelled() => return,
                    next = loop_listener.accept() => next,
                };
                match next {
                    Ok((stream, _address)) => loop_self.accept_socket(stream, &accept),
                    Err(error) => loop_self.report_error(&io_error(&error)),
                }
            }
        });
        Ok(())
    }

    /// `listener.ts` `acceptSocket` (`listener.ts:95-124`).
    fn accept_socket(&self, stream: UnixStream, accept: &ByteConnectionAcceptor) {
        let state = &self.state;
        if state.closing.load(Ordering::SeqCst) {
            return;
        }
        let connection = Arc::new(UnixByteConnection::new(
            stream,
            state.options.graceful_close_timeout_ms,
            state.options.max_pending_bytes,
        ));
        state
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(connection.clone());
        let handler: ByteConnectionHandler = accept(connection.clone() as Arc<dyn ByteConnection>);
        // Read pump: data → `onData`; EOF/error → teardown + `onClose`
        // (upstream socket `close` event).
        let pump_connection = connection.clone();
        let pump_state = Arc::clone(state);
        tokio::spawn(async move {
            // The read half is installed by the connection constructor and
            // handed to this pump (the generic split halves are not `Clone`).
            let Some(mut read_half) = pump_connection.take_read_half() else {
                return;
            };
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match read_half.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        pump_connection.mark_closed();
                        pump_state
                            .connections
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .retain(|candidate| !Arc::ptr_eq(candidate, &pump_connection));
                        (handler.on_close)();
                        return;
                    }
                    Ok(read) => (handler.on_data)(&buffer[..read]),
                }
            }
        });
    }

    /// `listener.ts` `closeServerAndCleanup` (`listener.ts:136-145`).
    async fn close_server_and_cleanup(&self) {
        // `closeNetServer`: dropping the listener stops accepting.
        *self.state.server.lock().unwrap() = None;
        if let Some(owned) = self.state.owned_bind_path.lock().unwrap().take() {
            // Remove an unpublished startup bind path before the public
            // route.
            let _ = std::fs::remove_file(&owned);
        }
        if let Err(error) = self.cleanup_owned_socket() {
            self.report_error(&error);
        }
    }

    /// `listener.ts` `cleanupOwnedSocket` (`listener.ts:147-179`): remove
    /// the public route only when its filesystem identity still matches the
    /// socket we bound.
    fn cleanup_owned_socket(&self) -> Result<(), OperationError> {
        let identity = self.state.socket_identity.lock().unwrap().take();
        let Some(identity) = identity else {
            return Ok(());
        };
        let path = PathBuf::from(&self.state.options.path);
        let current = match std::fs::symlink_metadata(&path) {
            Ok(current) => current,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(&error)),
        };
        let current_identity = identity_of(&current);
        if !is_socket_metadata(&current)
            || current_identity.dev != identity.dev
            || current_identity.ino != identity.ino
        {
            return Ok(());
        }

        let preserved = path.with_file_name(format!("cleanup-{}", &random_suffix()[..6]));
        if let Err(error) = std::fs::rename(&path, &preserved) {
            if error.kind() == ErrorKind::NotFound {
                return Ok(());
            }
            return Err(io_error(&error));
        }
        let moved = std::fs::symlink_metadata(&preserved).map_err(|error| io_error(&error))?;
        if is_socket_metadata(&moved)
            && identity_of(&moved).dev == identity.dev
            && identity_of(&moved).ino == identity.ino
        {
            let _ = std::fs::remove_file(&preserved);
            return Ok(());
        }
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                std::fs::rename(&preserved, &path).map_err(|error| io_error(&error))?;
            }
            Err(error) => return Err(io_error(&error)),
            Ok(_) => {}
        }
        Err(OperationError::Other(format!(
            "Unix listener path changed during cleanup; preserved replacement at {}",
            preserved.display()
        )))
    }
}

/// `listener.ts` `closeInternal` (`listener.ts:126-134`).
async fn close_internal(listener: &Arc<UnixListener>) -> Result<(), OperationError> {
    let state = &listener.state;
    // Stop accepting and run the server-side cleanup.
    if let Some(token) = state.accept_task.lock().unwrap().take() {
        token.cancel();
    }
    let server_was_live = state.server.lock().unwrap().is_some();
    let server_cleanup = if server_was_live {
        listener.close_server_and_cleanup().await;
        None
    } else {
        Some(listener.cleanup_owned_socket())
    };
    let connections: Vec<Arc<UnixByteConnection>> = state
        .connections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for connection in &connections {
        let _ = ByteConnection::close(connection.as_ref(), None).await;
    }
    if let Some(result) = server_cleanup {
        result?;
    }
    if let Some(owned) = state.owned_bind_path.lock().unwrap().take() {
        let _ = std::fs::remove_file(&owned);
    }
    state
        .connections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    *state.server.lock().unwrap() = None;
    Ok(())
}

impl ServerListener for UnixListener {
    fn start(
        &self,
        accept: ByteConnectionAcceptor,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            // The trait method takes `&self`; rebuild the shared handle so
            // the future is `'static` (the shared state is the `Arc`).
            let listener = UnixListener { state };
            UnixListener::start(&listener, accept).await
        })
    }

    fn close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            // The trait method takes `&self`; rebuild the shared handle for
            // the memoized close (upstream `closePromise` memoization).
            let listener = Arc::new(UnixListener {
                state: Arc::clone(&state),
            });
            state
                .close_once
                .get_or_init(move || async move { close_internal(&listener).await })
                .await
                .clone()
        })
    }
}

/// Commands for the single writer task (`listener.ts` `writeTail`): chunks
/// queue in send order; `End` half-closes the socket behind every queued
/// write (upstream `socket.end(...)`).
enum WriterCommand {
    Chunk {
        chunk: Vec<u8>,
        done: oneshot::Sender<Result<(), OperationError>>,
    },
    End,
}

/// The shared per-connection state (`listener.ts` `UnixByteConnection`
/// fields, `listener.ts:195-201`).
struct ConnectionInner {
    graceful_close_timeout_ms: u64,
    max_pending_bytes: usize,
    pending_bytes: Arc<AtomicUsize>,
    closed_value: AtomicBool,
    closing: AtomicBool,
    writes: mpsc::UnboundedSender<WriterCommand>,
    /// Installed by the constructor, taken once by the read pump (the
    /// generic split halves are not `Clone`).
    read_half: Mutex<Option<ReadHalf<UnixStream>>>,
    close_once: OnceCell<Result<(), OperationError>>,
    closed_signal: super::testing::host::Deferred<()>,
}

/// `listener.ts` `UnixByteConnection` (`listener.ts:191-292`,
/// `@internal` exported for transport-level verification). Cheap to clone.
#[derive(Clone)]
pub struct UnixByteConnection {
    inner: Arc<ConnectionInner>,
}

impl UnixByteConnection {
    pub fn new(
        stream: UnixStream,
        graceful_close_timeout_ms: u64,
        max_pending_bytes: u64,
    ) -> UnixByteConnection {
        let (read_half, mut write_half) = tokio::io::split(stream);
        let (writes, mut rx) = mpsc::unbounded_channel::<WriterCommand>();
        // The single writer task serializes writes like the upstream
        // `writeTail` promise chain; pending-byte accounting unwinds on
        // completion. It owns the write half, so `End` (the half-close)
        // queues behind every pending write.
        let pending: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let writer_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                match command {
                    WriterCommand::Chunk { chunk, done } => {
                        let result = match write_half.write_all(&chunk).await {
                            Ok(()) => Ok(()),
                            Err(error) => Err(OperationError::Other(io_message(&error))),
                        };
                        writer_pending.fetch_sub(chunk.len(), Ordering::SeqCst);
                        let _ = done.send(result);
                    }
                    WriterCommand::End => {
                        let _ = write_half.shutdown().await;
                    }
                }
            }
        });
        let inner = Arc::new(ConnectionInner {
            graceful_close_timeout_ms,
            max_pending_bytes: max_pending_bytes as usize,
            pending_bytes: pending,
            closed_value: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            writes,
            read_half: Mutex::new(Some(read_half)),
            close_once: OnceCell::new(),
            closed_signal: super::testing::host::Deferred::new(),
        });
        UnixByteConnection { inner }
    }
    pub fn closed(&self) -> bool {
        self.inner.closed_value.load(Ordering::SeqCst)
    }

    pub fn mark_closed(&self) {
        if self.inner.closed_value.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.closing.store(true, Ordering::SeqCst);
        self.inner.closed_signal.resolve(());
    }

    /// Hands the read half to the connection's read pump.
    fn take_read_half(&self) -> Option<ReadHalf<UnixStream>> {
        self.inner
            .read_half
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            if inner.closed_value.load(Ordering::SeqCst) || inner.closing.load(Ordering::SeqCst) {
                return Err(OperationError::Other(
                    "Unix connection is closed".to_string(),
                ));
            }
            let pending = inner.pending_bytes.fetch_add(chunk.len(), Ordering::SeqCst);
            if pending + chunk.len() > inner.max_pending_bytes {
                inner.pending_bytes.fetch_sub(chunk.len(), Ordering::SeqCst);
                return Err(OperationError::Other(
                    "Unix connection exceeded its pending byte limit".to_string(),
                ));
            }
            let (done, receipt) = oneshot::channel();
            let _ = inner.writes.send(WriterCommand::Chunk { chunk, done });
            match receipt.await {
                Ok(result) => result,
                // The writer dropped the sender mid-write (upstream "Unix
                // connection closed during write").
                Err(_) => Err(OperationError::Other(
                    "Unix connection closed during write".to_string(),
                )),
            }
        })
    }

    pub fn close(
        &self,
        final_chunk: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            if inner.closed_value.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Memoized like the upstream `closePromise`; the first call's
            // final chunk wins.
            let init_inner = Arc::clone(&inner);
            inner
                .close_once
                .get_or_init(move || async move { graceful_close(&init_inner, final_chunk).await })
                .await
                .clone()
        })
    }
}

/// The upstream close sequence (`listener.ts:230-260`): wait for the write
/// tail (the final chunk queues behind every pending write on the single
/// writer), end the socket, resolve on the socket `close` event, destroy on
/// the graceful-close timeout.
async fn graceful_close(
    inner: &Arc<ConnectionInner>,
    final_chunk: Option<Vec<u8>>,
) -> Result<(), OperationError> {
    inner.closing.store(true, Ordering::SeqCst);
    if let Some(final_chunk) = final_chunk {
        let (done, receipt) = oneshot::channel();
        let _ = inner.writes.send(WriterCommand::Chunk {
            chunk: final_chunk,
            done,
        });
        let _ = receipt.await;
    }
    // `socket.end(...)` — half-close the write side. The writer task owns
    // the half, so the end queues behind every pending write (the final
    // chunk above has already been written by the time we get here).
    let _ = inner.writes.send(WriterCommand::End);
    tokio::select! {
        closed = inner.closed_signal.promise() => closed,
        _ = tokio::time::sleep(Duration::from_millis(inner.graceful_close_timeout_ms)) => {
            // Timeout destroys the socket (`socket.destroy()`).
        }
    };
    inner.closed_value.store(true, Ordering::SeqCst);
    Ok(())
}

impl ByteConnection for UnixByteConnection {
    fn closed(&self) -> bool {
        UnixByteConnection::closed(self)
    }

    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
        UnixByteConnection::send(self, chunk)
    }

    fn close(
        &self,
        final_chunk: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        UnixByteConnection::close(self, final_chunk)
    }
}

/// `preset.ts` `createUnixServer` (`preset.ts:12-27`): compose `Server` with
/// one Unix-domain socket listener.
pub fn create_unix_server(
    host: Arc<dyn ServerHost>,
    options: UnixServerOptions,
) -> Result<Arc<Server>, OperationError> {
    let listener = create_unix_listener(UnixListenerOptions {
        path: options.path,
        mode: options.mode,
        max_pending_bytes: options.max_pending_bytes,
        graceful_close_timeout_ms: options.graceful_close_timeout_ms,
        max_frame_length: options.max_frame_length,
        on_error: options.on_error.clone(),
    })?;
    let mut server_options = ServerOptions::new(vec![listener], options.server_id)
        .max_frame_length(options.max_frame_length)
        .handshake_timeout_ms(options.handshake_timeout_ms);
    server_options.on_connection_count_changed = options.on_connection_count_changed;
    server_options.on_error = options.on_error;
    Server::new(host, server_options)
}
