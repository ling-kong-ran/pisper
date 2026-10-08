//! Port of `packages/client/src/unix.ts` (299 lines): Unix-domain-socket byte
//! transports and local server discovery for the client package.
//!
//! `cfg(unix)` — upstream gates the same surface ("Unix transport is not
//! supported on Windows", `unix.ts:38,99`), and its own unix test files run
//! `describe.runIf(process.platform !== "win32")`. This module therefore
//! cannot compile on the Windows development host (disclosed seam S6); the
//! ported discovery/transport behavior mirrors `unix.ts` line-for-line:
//! validation texts, the omitted-probe predicate (`unix.ts:260-276`), the
//! 16-probe concurrency cap, server-id sort order, and the pending-byte
//! write queue (upstream `writeTail` + `maxPendingBytes`).

use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::protocol::framing::DEFAULT_MAX_FRAME_LENGTH;
use crate::protocol::protocol::{is_server_id, ServerId};

use super::client::Client;
use super::errors::{ClientError, TransportError};
use super::transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};
use super::types::ClientOptions;

/// `unix.ts:14-17`.
const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;
const UNIX_SOCKET_SUFFIX: &str = ".sock";
const MAX_CONCURRENT_DISCOVERY_PROBES: usize = 16;

/// `unix.ts:19-22`.
pub struct UnixTransportOptions {
    pub path: String,
    pub max_pending_bytes: Option<u64>,
}

/// `unix.ts:24-27`.
#[derive(Clone, Debug)]
pub struct UnixServerRoute {
    pub server_id: ServerId,
    pub path: String,
}

/// `unix.ts:29-34`.
pub struct DiscoverUnixServersOptions {
    /// Directory containing server-addressed Unix sockets.
    pub directory: String,
    /// Maximum time for each connection and handshake. Defaults to 1,000 ms.
    pub timeout_ms: Option<u64>,
}

/// `unix.ts:93-101` (`validateUnixTransportOptions`; the win32 branch is the
/// module's `cfg(unix)` gate).
fn validate_unix_transport_options(options: &UnixTransportOptions) -> Result<u64, ClientError> {
    if options.path.is_empty() {
        return Err(ClientError::Type(
            "Unix transport path must not be empty".to_string(),
        ));
    }
    let max_pending_bytes = options
        .max_pending_bytes
        .unwrap_or(DEFAULT_MAX_FRAME_LENGTH * 4);
    if max_pending_bytes == 0 {
        return Err(ClientError::Type(
            "Unix transport maxPendingBytes must be a positive safe integer".to_string(),
        ));
    }
    Ok(max_pending_bytes)
}

fn io_transport_error(error: &std::io::Error) -> ClientError {
    ClientError::Transport(TransportError {
        message: io_message(error),
        code: io_code(error),
    })
}

fn io_message(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{error} (os error {code})"),
        None => error.to_string(),
    }
}

/// The node error `code`s the upstream discovery predicate inspects
/// (`unix.ts:269-273`).
fn io_code(error: &std::io::Error) -> Option<String> {
    let name = match error.raw_os_error()? {
        2 => "ENOENT",
        13 => "EACCES",
        20 => "ENOTDIR",
        32 => "EPIPE",
        36 => "ENAMETOOLONG",
        40 => "ELOOP",
        104 => "ECONNRESET",
        110 => "ETIMEDOUT",
        111 => "ECONNREFUSED",
        _ => return None,
    };
    Some(name.to_string())
}

/// Discovers reachable local servers by probing server-addressed Unix
/// sockets (`unix.ts:37-85`).
pub async fn discover_unix_servers(
    options: DiscoverUnixServersOptions,
) -> Result<Vec<UnixServerRoute>, ClientError> {
    let directory = options.directory;
    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_DISCOVERY_TIMEOUT_MS);
    if timeout_ms == 0 || timeout_ms > MAX_TIMER_DELAY_MS {
        return Err(ClientError::Type(format!(
            "Unix discovery timeoutMs must be an integer between 1 and {MAX_TIMER_DELAY_MS}"
        )));
    }

    let names: Vec<String> = match std::fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        // `isErrorCode(error, "ENOENT")` → `[]`.
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_transport_error(&error)),
    };

    let candidates: Vec<UnixServerRoute> = names
        .iter()
        .filter(|name| name.ends_with(UNIX_SOCKET_SUFFIX))
        .filter_map(|name| {
            let server_id = name[..name.len() - UNIX_SOCKET_SUFFIX.len()].to_string();
            if is_server_id(&server_id) {
                Some(UnixServerRoute {
                    server_id,
                    path: PathBuf::from(&directory)
                        .join(name)
                        .to_string_lossy()
                        .into_owned(),
                })
            } else {
                None
            }
        })
        .collect();

    let routes: Arc<std::sync::Mutex<Vec<UnixServerRoute>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let failure: Arc<std::sync::Mutex<Option<ClientError>>> = Arc::new(std::sync::Mutex::new(None));
    let next_index = Arc::new(AtomicUsize::new(0));
    let candidates = Arc::new(std::sync::Mutex::new(candidates));
    let worker_count = MAX_CONCURRENT_DISCOVERY_PROBES.min(candidates.lock().unwrap().len());

    let mut workers = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let candidates = candidates.clone();
        let routes = routes.clone();
        let failure = failure.clone();
        let next_index = next_index.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if failure.lock().unwrap().is_some() {
                    return;
                }
                let candidate = {
                    let guard = candidates.lock().unwrap();
                    let index = next_index.fetch_add(1, Ordering::SeqCst);
                    if index >= guard.len() {
                        return;
                    }
                    guard[index].clone()
                };
                // A socket can disappear between readdir and lstat during
                // normal server shutdown (`unix.ts:71-73`).
                match std::fs::symlink_metadata(&candidate.path) {
                    Ok(metadata) if metadata.file_type().is_socket() => {}
                    Ok(_) => continue,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => {
                        let mut slot = failure.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(io_transport_error(&error));
                        }
                        return;
                    }
                }
                match probe_unix_server(&candidate, timeout_ms).await {
                    Ok(reachable) => {
                        if reachable {
                            routes.lock().unwrap().push(candidate);
                        }
                    }
                    Err(error) => {
                        let mut slot = failure.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                        return;
                    }
                }
            }
        }));
    }
    for worker in workers {
        let _ = worker.await;
    }
    if let Some(error) = failure.lock().unwrap().take() {
        return Err(error);
    }
    // `left.serverId.localeCompare(right.serverId)`: canonical UUIDv4 ids
    // are ASCII, so byte order is the collation order.
    let mut routes = routes.lock().unwrap().clone();
    routes.sort_by(|left, right| left.server_id.cmp(&right.server_id));
    Ok(routes)
}

/// Creates fresh Unix-domain socket transports for Client connection attempts
/// (`unix.ts:87-91`).
pub fn create_unix_transport_factory(
    options: UnixTransportOptions,
) -> Result<ByteTransportFactory, ClientError> {
    let max_pending_bytes = validate_unix_transport_options(&options)?;
    let path = options.path;
    Ok(Arc::new(move |handlers: ByteTransportHandlers| {
        connect_unix_socket(path.clone(), max_pending_bytes, handlers)
    }))
}

/// `unix.ts:103-145` (`connectUnixSocket`): the transport resolves once the
/// socket is connected; a terminal failure before that rejects the factory.
fn connect_unix_socket(
    path: String,
    max_pending_bytes: u64,
    handlers: ByteTransportHandlers,
) -> BoxFuture<'static, Result<Arc<dyn ByteTransport>, ClientError>> {
    Box::pin(async move {
        let stream = match UnixStream::connect(&path).await {
            Ok(stream) => stream,
            Err(error) => return Err(io_transport_error(&error)),
        };
        Ok(
            Arc::new(UnixByteTransport::new(stream, max_pending_bytes, handlers))
                as Arc<dyn ByteTransport>,
        )
    })
}

struct WriteCommand {
    chunk: Vec<u8>,
    permit: tokio::sync::OwnedSemaphorePermit,
    done: oneshot::Sender<Result<(), ClientError>>,
}

/// `unix.ts:147-235` (`UnixByteTransport`): writes are serialized through a
/// writer task (upstream `#writeTail`), the pending-byte limit is enforced at
/// `send` call time (upstream `#pendingBytes`), and a local `close()` never
/// reports `onClose` (upstream `markLocalClose` gates the outer close
/// handler).
struct UnixByteTransport {
    writes: mpsc::UnboundedSender<WriteCommand>,
    pending: Arc<Semaphore>,
    closed: Arc<AtomicBool>,
    /// Wakes the writer and read pump on local close; dropping both socket
    /// halves ends the socket (upstream `socket.destroy()`).
    destroy: CancellationToken,
}

impl UnixByteTransport {
    fn new(
        stream: UnixStream,
        max_pending_bytes: u64,
        handlers: ByteTransportHandlers,
    ) -> UnixByteTransport {
        let pending = Arc::new(Semaphore::new(
            u32::try_from(max_pending_bytes).unwrap_or(u32::MAX) as usize,
        ));
        let closed = Arc::new(AtomicBool::new(false));
        let destroy = CancellationToken::new();
        let (writes, mut rx) = mpsc::unbounded_channel::<WriteCommand>();
        let (read_half, mut write_half) = stream.into_split();

        // Writer task: one chunk at a time, in send order.
        let write_closed = closed.clone();
        let writer_destroy = destroy.clone();
        tokio::spawn(async move {
            loop {
                let command = tokio::select! {
                    _ = writer_destroy.cancelled() => return,
                    command = rx.recv() => match command {
                        Some(command) => command,
                        None => return,
                    },
                };
                let WriteCommand {
                    chunk,
                    permit,
                    done,
                } = command;
                if write_closed.load(Ordering::SeqCst) {
                    let _ = done.send(Err(ClientError::Transport(TransportError::new(
                        "Unix transport is closed",
                    ))));
                    continue;
                }
                let result = write_half.write_all(&chunk).await;
                drop(permit);
                let _ = match result {
                    Ok(()) => done.send(Ok(())),
                    Err(error) => done.send(Err(io_transport_error(&error))),
                };
            }
        });

        // Read pump: delivers inbound bytes; a peer close reports `onClose`
        // exactly once and only when the close was not local.
        let read_closed = closed.clone();
        let pump_handlers = handlers.clone();
        let pump_destroy = destroy.clone();
        tokio::spawn(async move {
            let mut read_half = read_half;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let read = tokio::select! {
                    _ = pump_destroy.cancelled() => return,
                    read = read_half.read(&mut buffer) => read,
                };
                match read {
                    Ok(0) => {
                        if !read_closed.load(Ordering::SeqCst) {
                            (pump_handlers.on_close)();
                        }
                        return;
                    }
                    Ok(read) => (pump_handlers.on_data)(&buffer[..read]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                    Err(error) => {
                        if !read_closed.swap(true, Ordering::SeqCst) {
                            (pump_handlers.on_error)(io_transport_error(&error));
                        }
                        return;
                    }
                }
            }
        });

        UnixByteTransport {
            writes,
            pending,
            closed,
            destroy,
        }
    }
}

impl ByteTransport for UnixByteTransport {
    /// `unix.ts:161-177`: the pending-byte accounting happens at call time;
    /// the returned future resolves when the write completes.
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), ClientError>> {
        if self.closed.load(Ordering::SeqCst) {
            return Box::pin(async move {
                Err(ClientError::Transport(TransportError::new(
                    "Unix transport is closed",
                )))
            });
        }
        let permit = match self
            .pending
            .clone()
            .try_acquire_many_owned(chunk.len() as u32)
        {
            Ok(permit) => permit,
            Err(_) => {
                return Box::pin(async move {
                    Err(ClientError::Transport(TransportError::new(
                        "Unix transport exceeded its pending byte limit",
                    )))
                });
            }
        };
        let (done, receipt) = oneshot::channel();
        let _ = self.writes.send(WriteCommand {
            chunk,
            permit,
            done,
        });
        Box::pin(async move {
            match receipt.await {
                Ok(result) => result,
                // The writer dropped the sender mid-write (upstream
                // "Unix transport closed during write").
                Err(_) => Err(ClientError::Transport(TransportError::new(
                    "Unix transport closed during write",
                ))),
            }
        })
    }

    /// `unix.ts:179-184`.
    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Destroy: wake the writer and the read pump; dropping both socket
        // halves ends the socket (the write half shuts the write direction
        // down on drop).
        self.destroy.cancel();
    }
}

/// Whether a failed probe is "stale or shutting down" (`unix.ts:260-276`):
/// timeouts, protocol failures, plain disconnects, version rejections, and
/// the expected IO codes are omitted; anything else propagates.
fn probe_is_omittable(error: &ClientError) -> bool {
    if matches!(error, ClientError::Protocol(_)) {
        return true;
    }
    if let ClientError::Disconnected(disconnected) = error {
        if disconnected.cause.is_none() {
            return true;
        }
    }
    if let ClientError::Server(server) = error {
        if server.code == "version" {
            return true;
        }
    }
    ["ENOENT", "ECONNREFUSED", "ECONNRESET", "EPIPE", "ETIMEDOUT"]
        .iter()
        .any(|code| error.error_code_is(code))
}

/// `unix.ts:237-286` (`probeUnixServer`): full connect + handshake with a
/// timeout; `Ok(true)` reachable, `Ok(false)` omitted, `Err` propagates.
async fn probe_unix_server(route: &UnixServerRoute, timeout_ms: u64) -> Result<bool, ClientError> {
    let max_pending_bytes = validate_unix_transport_options(&UnixTransportOptions {
        path: route.path.clone(),
        max_pending_bytes: None,
    })?;
    let path = route.path.clone();
    let client = Client::new(ClientOptions::new(
        Arc::new(move |handlers: ByteTransportHandlers| {
            connect_unix_socket(path.clone(), max_pending_bytes, handlers)
        }),
        route.server_id.clone(),
    ))?;
    let connect = client.connect();
    let outcome = tokio::time::timeout(Duration::from_millis(timeout_ms), connect).await;
    let reachable = match outcome {
        // The probe socket is destroyed and the route omitted on timeout
        // (`unix.ts:250-257`).
        Err(_elapsed) => false,
        Ok(receipt) => match receipt {
            Ok(Ok(_hello)) => true,
            Ok(Err(error)) => {
                if probe_is_omittable(&error) {
                    false
                } else {
                    client.dispose().await;
                    return Err(error);
                }
            }
            // Dropped resolver: the client was disposed concurrently.
            Err(_) => false,
        },
    };
    client.dispose().await;
    Ok(reachable)
}
