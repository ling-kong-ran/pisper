//! D1 transport layer for the coordinator control plane: byte-line sockets,
//! listeners and connectors over interchangeable backends.
//!
//! Upstream `coordinator.ts` uses `node:net` `Server`/`Socket` on a
//! filesystem path (a Unix domain socket on POSIX, a named pipe on Windows).
//! The port splits this into:
//! - the [`ControlSocket`]/[`ControlListener`]/[`ControlConnector`] seams; and
//! - real backends per platform: `cfg(unix)` builds a true `UnixListener`/
//!   `UnixStream` backend ([`UnixControlListener`]); `cfg(windows)` builds a
//!   loopback TCP backend ([`TcpControlListener`]) because `std` on stable
//!   Windows exposes no named-pipe server (upstream's win32 mechanism). The
//!   upstream Unix-socket file semantics (stale-socket lstat/unlink, 0o600
//!   chmod) are already win32 no-ops upstream (`restrictSocket`/
//!   `cleanupSocket`), so the behavioral surface matches: same frames, same
//!   routing, same error strings; only the transport path shape differs and
//!   is disclosed here.
//! - an in-process [`memory`] backend used by the loopback tests and by
//!   embedders that already own both endpoints.

use crate::coding_agent::experimental::process::MAX_CONTROL_LINE_BYTES;

/// Upstream `attachJsonLineReader`/`attachRoutedLineReader` line decoding:
/// blocking read of one line including its `\n` terminator, `None`
/// on clean EOF. Oversized buffers fail with the exact upstream error via
/// [`read_line_capped`].
pub trait ControlSocket: Send {
    fn read_line(&mut self) -> std::io::Result<Option<String>>;
    /// Upstream `writeRoutedLine`/`writeJsonLine` (`socket.write(line)`).
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;
    /// Raw byte copy source for the public-path proxy
    /// (`client.pipe(upstream)`).
    fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    /// Raw byte copy sink for the public-path proxy.
    fn write_all_bytes(&mut self, buf: &[u8]) -> std::io::Result<()>;
    /// Upstream `socket.destroy()`.
    fn shutdown(&mut self);
    /// Independent handle over the same connection (the worker main loop
    /// needs a reader half and a writer half that never block each other).
    fn try_clone(&self) -> Option<Box<dyn ControlSocket>>;
}

/// Upstream `createServer(...).listen(path)`.
pub trait ControlListener: Send + Sync {
    fn accept(&self) -> std::io::Result<Box<dyn ControlSocket>>;
    fn path(&self) -> &str;
    /// Upstream `server.close()`.
    fn close(&self);
}

/// Upstream `createConnection(path)`.
pub trait ControlConnector: Send + Sync {
    fn connect(&self, path: &str) -> std::io::Result<Box<dyn ControlSocket>>;
}

/// Line reader with the upstream 128 MiB cap. `oversized_error` selects the
/// exact per-side message ("Coordinator message is too large" on the
/// coordinator and server-connection readers, "Session worker control
/// message is too large" on the worker's own reader).
pub fn read_line_capped(
    socket: &mut dyn ControlSocket,
    oversized_error: &str,
) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    loop {
        let Some(chunk) = socket.read_line()? else {
            // Upstream only dispatches newline-terminated frames.
            return Ok(None);
        };
        line.push_str(&chunk);
        if line.len() as u64 > MAX_CONTROL_LINE_BYTES {
            return Err(std::io::Error::other(oversized_error.to_string()));
        }
        if chunk.ends_with('\n') {
            // Upstream slices off the trailing newline before JSON.parse.
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

// ── Unix domain socket backend (upstream's POSIX mechanism) ────────────────

#[cfg(unix)]
mod unix_backend {
    use super::{ControlConnector, ControlListener, ControlSocket};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Mutex;

    pub struct UnixControlSocket {
        reader: Mutex<BufReader<UnixStream>>,
        writer: Mutex<UnixStream>,
    }

    impl UnixControlSocket {
        pub fn new(stream: UnixStream) -> Self {
            let read_half = stream.try_clone().expect("unix stream clone");
            UnixControlSocket {
                reader: Mutex::new(BufReader::new(read_half)),
                writer: Mutex::new(stream),
            }
        }
    }

    impl ControlSocket for UnixControlSocket {
        fn read_line(&mut self) -> std::io::Result<Option<String>> {
            let mut reader = self.reader.lock().unwrap();
            let mut line = String::new();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(None);
            }
            Ok(Some(line))
        }

        fn write_line(&mut self, line: &str) -> std::io::Result<()> {
            self.writer.lock().unwrap().write_all(line.as_bytes())
        }

        fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reader.lock().unwrap().read(buf)
        }

        fn write_all_bytes(&mut self, buf: &[u8]) -> std::io::Result<()> {
            self.writer.lock().unwrap().write_all(buf)
        }

        fn shutdown(&mut self) {
            if let Ok(writer) = self.writer.lock() {
                let _ = writer.shutdown(std::net::Shutdown::Both);
            }
        }

        fn try_clone(&self) -> Option<Box<dyn ControlSocket>> {
            let reader = self.reader.lock().ok()?.get_ref().try_clone().ok()?;
            let writer = self.writer.lock().ok()?.try_clone().ok()?;
            Some(Box::new(UnixControlSocket {
                reader: Mutex::new(BufReader::new(reader)),
                writer: Mutex::new(writer),
            }))
        }
    }

    pub struct UnixControlListener {
        listener: Mutex<Option<UnixListener>>,
        path: String,
    }

    impl UnixControlListener {
        pub fn bind(path: &str) -> std::io::Result<Self> {
            let listener = UnixListener::bind(path)?;
            listener.set_nonblocking(true)?;
            Ok(UnixControlListener {
                listener: Mutex::new(Some(listener)),
                path: path.to_owned(),
            })
        }

        /// Releases the filesystem path (upstream `cleanupSocket`).
        pub fn unlink(&self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    impl ControlListener for UnixControlListener {
        fn accept(&self) -> std::io::Result<Box<dyn ControlSocket>> {
            loop {
                let result = self
                    .listener
                    .lock()
                    .unwrap()
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("listener closed"))?
                    .accept();
                match result {
                    Ok((stream, _)) => return Ok(Box::new(UnixControlSocket::new(stream))),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        fn path(&self) -> &str {
            &self.path
        }

        fn close(&self) {
            self.listener.lock().unwrap().take();
            self.unlink();
        }
    }

    pub struct UnixControlConnector;

    impl ControlConnector for UnixControlConnector {
        fn connect(&self, path: &str) -> std::io::Result<Box<dyn ControlSocket>> {
            let stream = UnixStream::connect(path)?;
            Ok(Box::new(UnixControlSocket::new(stream)))
        }
    }
}

#[cfg(unix)]
pub use unix_backend::{UnixControlConnector, UnixControlListener, UnixControlSocket};

// ── Windows loopback TCP backend (disclosed stand-in for node named pipes) ─

#[cfg(not(unix))]
mod tcp_backend {
    use super::{ControlConnector, ControlListener, ControlSocket};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;

    /// Control "paths" on this backend are `tcp://127.0.0.1:<port>` strings;
    /// the coordinator handshake never interprets the path, so the frame and
    /// routing behavior is identical to the Unix backend.
    pub fn tcp_path(port: u16) -> String {
        format!("tcp://127.0.0.1:{port}")
    }

    fn parse_path(path: &str) -> std::io::Result<String> {
        path.strip_prefix("tcp://127.0.0.1:")
            .map(str::to_owned)
            .ok_or_else(|| {
                std::io::Error::other(format!("Coordinator path is not a socket: {path}"))
            })
    }

    pub struct TcpControlSocket {
        reader: Mutex<BufReader<TcpStream>>,
        writer: Mutex<TcpStream>,
    }

    impl TcpControlSocket {
        pub fn new(stream: TcpStream) -> Self {
            let read_half = stream.try_clone().expect("tcp stream clone");
            TcpControlSocket {
                reader: Mutex::new(BufReader::new(read_half)),
                writer: Mutex::new(stream),
            }
        }
    }

    impl ControlSocket for TcpControlSocket {
        fn read_line(&mut self) -> std::io::Result<Option<String>> {
            let mut reader = self.reader.lock().unwrap();
            let mut line = String::new();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(None);
            }
            Ok(Some(line))
        }

        fn write_line(&mut self, line: &str) -> std::io::Result<()> {
            self.writer.lock().unwrap().write_all(line.as_bytes())
        }

        fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reader.lock().unwrap().read(buf)
        }

        fn write_all_bytes(&mut self, buf: &[u8]) -> std::io::Result<()> {
            self.writer.lock().unwrap().write_all(buf)
        }

        fn shutdown(&mut self) {
            if let Ok(writer) = self.writer.lock() {
                let _ = writer.shutdown(std::net::Shutdown::Both);
            }
        }

        fn try_clone(&self) -> Option<Box<dyn ControlSocket>> {
            let reader = self.reader.lock().ok()?.get_ref().try_clone().ok()?;
            let writer = self.writer.lock().ok()?.try_clone().ok()?;
            Some(Box::new(TcpControlSocket {
                reader: Mutex::new(BufReader::new(reader)),
                writer: Mutex::new(writer),
            }))
        }
    }

    pub struct TcpControlListener {
        inner: Mutex<Option<TcpListener>>,
        path: String,
    }

    impl TcpControlListener {
        pub fn bind(path: &str) -> std::io::Result<Self> {
            let port: u16 = parse_path(path)?.parse().map_err(|error| {
                std::io::Error::other(format!("Coordinator path is not a socket: {error}"))
            })?;
            let listener = TcpListener::bind(("127.0.0.1", port))?;
            listener.set_nonblocking(true)?;
            Ok(TcpControlListener {
                inner: Mutex::new(Some(listener)),
                path: path.to_owned(),
            })
        }

        /// Binds an ephemeral loopback port and reports its control path
        /// (used by tests that do not care about the exact endpoint).
        pub fn bind_ephemeral() -> std::io::Result<Self> {
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            listener.set_nonblocking(true)?;
            Ok(TcpControlListener {
                inner: Mutex::new(Some(listener)),
                path: tcp_path(port),
            })
        }
    }

    impl ControlListener for TcpControlListener {
        fn accept(&self) -> std::io::Result<Box<dyn ControlSocket>> {
            loop {
                let result = self
                    .inner
                    .lock()
                    .unwrap()
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("listener closed"))?
                    .accept();
                match result {
                    Ok((stream, _)) => return Ok(Box::new(TcpControlSocket::new(stream))),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        fn path(&self) -> &str {
            &self.path
        }

        fn close(&self) {
            self.inner.lock().unwrap().take();
        }
    }

    pub struct TcpControlConnector;

    impl ControlConnector for TcpControlConnector {
        fn connect(&self, path: &str) -> std::io::Result<Box<dyn ControlSocket>> {
            let port: u16 = parse_path(path)?.parse().map_err(|error| {
                std::io::Error::other(format!("Coordinator path is not a socket: {error}"))
            })?;
            let stream = TcpStream::connect(("127.0.0.1", port))?;
            Ok(Box::new(TcpControlSocket::new(stream)))
        }
    }
}

#[cfg(not(unix))]
pub use tcp_backend::{tcp_path, TcpControlConnector, TcpControlListener, TcpControlSocket};

/// Path-shaped constructor shared by the platform shells: Unix binds a domain
/// socket, Windows binds a loopback TCP port (the path must carry the
/// `tcp://127.0.0.1:<port>` shape; any other path binds an ephemeral port and
/// the listener reports the actual path).
pub fn bind_platform_listener(path: &str) -> std::io::Result<Box<dyn ControlListener>> {
    #[cfg(unix)]
    {
        UnixControlListener::bind(path)
            .map(|listener| Box::new(listener) as Box<dyn ControlListener>)
    }
    #[cfg(not(unix))]
    {
        if path.starts_with("tcp://127.0.0.1:") {
            TcpControlListener::bind(path)
                .map(|listener| Box::new(listener) as Box<dyn ControlListener>)
        } else {
            TcpControlListener::bind_ephemeral()
                .map(|listener| Box::new(listener) as Box<dyn ControlListener>)
        }
    }
}

/// Path-shaped connector shared by the platform shells.
pub fn platform_connector() -> Box<dyn ControlConnector> {
    #[cfg(unix)]
    {
        Box::new(UnixControlConnector)
    }
    #[cfg(not(unix))]
    {
        Box::new(TcpControlConnector)
    }
}

// ── In-memory backend (tests and in-process embedders) ─────────────────────

pub mod memory {
    use super::{ControlConnector, ControlListener, ControlSocket};
    use std::collections::{HashMap, VecDeque};
    use std::io;
    use std::sync::{Arc, Condvar, Mutex};

    #[derive(Default)]
    struct Pipe {
        closed: bool,
        bytes: Vec<u8>,
    }

    /// One direction of an in-memory socket pair: a byte pipe with close
    /// semantics (reads drain, then return EOF).
    struct PipeHalf {
        pipe: Mutex<Pipe>,
        signal: Condvar,
    }

    impl PipeHalf {
        fn new() -> Arc<PipeHalf> {
            Arc::new(PipeHalf {
                pipe: Mutex::new(Pipe::default()),
                signal: Condvar::new(),
            })
        }

        fn push(self: &Arc<Self>, data: &[u8]) -> io::Result<()> {
            let mut pipe = self.pipe.lock().unwrap();
            if pipe.closed {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"));
            }
            pipe.bytes.extend_from_slice(data);
            self.signal.notify_all();
            Ok(())
        }

        fn close(self: &Arc<Self>) {
            let mut pipe = self.pipe.lock().unwrap();
            pipe.closed = true;
            self.signal.notify_all();
        }

        /// Blocking pull into `buf`; returns 0 on a closed, drained pipe.
        fn pull(self: &Arc<Self>, buf: &mut [u8]) -> io::Result<usize> {
            let mut pipe = self.pipe.lock().unwrap();
            loop {
                if !pipe.bytes.is_empty() {
                    let take = pipe.bytes.len().min(buf.len());
                    let drained: Vec<u8> = pipe.bytes.drain(..take).collect();
                    buf[..take].copy_from_slice(&drained);
                    return Ok(take);
                }
                if pipe.closed {
                    return Ok(0);
                }
                pipe = self.signal.wait(pipe).unwrap();
            }
        }
    }

    // Clones split reading/writing but dropping one half must not disconnect.
    // The final owner closes both directions and wakes parked readers.
    struct MemoryEndpoint {
        local: Arc<PipeHalf>,
        peer: Arc<PipeHalf>,
    }
    impl Drop for MemoryEndpoint {
        fn drop(&mut self) {
            self.local.close();
            self.peer.close();
        }
    }

    pub struct MemorySocket {
        local: Arc<PipeHalf>,
        peer: Arc<PipeHalf>,
        pending: Mutex<Vec<u8>>,
        shut_down: Arc<Mutex<bool>>,
        owner: Arc<MemoryEndpoint>,
    }

    impl MemorySocket {
        fn pair() -> (MemorySocket, MemorySocket) {
            let a = PipeHalf::new();
            let b = PipeHalf::new();
            fn endpoint(local: Arc<PipeHalf>, peer: Arc<PipeHalf>) -> MemorySocket {
                MemorySocket {
                    pending: Mutex::new(Vec::new()),
                    shut_down: Arc::new(Mutex::new(false)),
                    owner: Arc::new(MemoryEndpoint {
                        local: local.clone(),
                        peer: peer.clone(),
                    }),
                    local,
                    peer,
                }
            }
            (endpoint(a.clone(), b.clone()), endpoint(b, a))
        }

        /// Pull with an internal carry buffer so partial reads never lose
        /// bytes.
        fn pull(&self, buf: &mut [u8]) -> io::Result<usize> {
            {
                let mut pending = self.pending.lock().unwrap();
                if !pending.is_empty() {
                    let take = pending.len().min(buf.len());
                    let drained: Vec<u8> = pending.drain(..take).collect();
                    buf[..take].copy_from_slice(&drained);
                    return Ok(take);
                }
            }
            let read = self.local.pull(buf)?;
            if read == 0 {
                return Ok(0);
            }
            let mut pending = self.pending.lock().unwrap();
            pending.extend_from_slice(&buf[..read]);
            let take = pending.len().min(buf.len());
            let drained: Vec<u8> = pending.drain(..take).collect();
            buf[..take].copy_from_slice(&drained);
            Ok(take)
        }

        fn write_raw(&self, data: &[u8]) -> io::Result<()> {
            if *self.shut_down.lock().unwrap() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"));
            }
            self.peer.push(data)
        }
    }

    impl ControlSocket for MemorySocket {
        fn read_line(&mut self) -> io::Result<Option<String>> {
            let mut byte = [0u8; 1];
            let mut line: Vec<u8> = Vec::new();
            loop {
                let read = self.pull(&mut byte)?;
                if read == 0 {
                    return Ok(if line.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&line).into_owned())
                    });
                }
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
                }
            }
        }

        fn write_line(&mut self, line: &str) -> io::Result<()> {
            self.write_raw(line.as_bytes())
        }

        fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.pull(buf)
        }

        fn write_all_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
            self.write_raw(buf)
        }

        fn shutdown(&mut self) {
            *self.shut_down.lock().unwrap() = true;
            self.local.close();
            self.peer.close();
        }

        fn try_clone(&self) -> Option<Box<dyn ControlSocket>> {
            // Shares the same pipe halves; only one clone should read.
            Some(Box::new(MemorySocket {
                local: Arc::clone(&self.local),
                peer: Arc::clone(&self.peer),
                pending: Mutex::new(Vec::new()),
                shut_down: self.shut_down.clone(),
                owner: self.owner.clone(),
            }))
        }
    }

    struct HubEntry {
        /// Parked server halves awaiting `accept`, FIFO.
        pending: Mutex<VecDeque<MemorySocket>>,
        available: Condvar,
        bound: std::sync::atomic::AtomicBool,
    }

    /// Named in-process bind point: `bind` parks a virtual listener for a
    /// path, `connect` produces the client half of a socket pair and wakes
    /// the parked accept.
    pub struct MemoryHub {
        entries: Mutex<HashMap<String, Arc<HubEntry>>>,
    }

    impl Default for MemoryHub {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MemoryHub {
        pub fn new() -> Self {
            MemoryHub {
                entries: Mutex::new(HashMap::new()),
            }
        }

        pub fn bind(self: &Arc<Self>, path: &str) -> io::Result<MemoryListener> {
            let mut entries = self.entries.lock().unwrap();
            if entries.contains_key(path) {
                return Err(io::Error::other(format!(
                    "Coordinator socket is already active: {path}"
                )));
            }
            entries.insert(
                path.to_owned(),
                Arc::new(HubEntry {
                    pending: Mutex::new(VecDeque::new()),
                    available: Condvar::new(),
                    bound: std::sync::atomic::AtomicBool::new(true),
                }),
            );
            Ok(MemoryListener {
                hub: Arc::clone(self),
                path: path.to_owned(),
            })
        }

        fn connect(self: &Arc<Self>, path: &str) -> io::Result<Box<dyn ControlSocket>> {
            let entry = {
                let entries = self.entries.lock().unwrap();
                entries.get(path).map(Arc::clone)
            };
            let Some(entry) = entry else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "connect ENOENT"));
            };
            if !entry.bound.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "connect ECONNREFUSED",
                ));
            }
            let (client, server) = MemorySocket::pair();
            entry.pending.lock().unwrap().push_back(server);
            entry.available.notify_one();
            Ok(Box::new(client))
        }

        fn take_pending(&self, path: &str) -> Option<MemorySocket> {
            let entry = {
                let entries = self.entries.lock().unwrap();
                entries.get(path).map(Arc::clone)
            }?;
            let mut pending = entry.pending.lock().unwrap();
            loop {
                if let Some(socket) = pending.pop_front() {
                    return Some(socket);
                }
                if !entry.bound.load(std::sync::atomic::Ordering::SeqCst) {
                    return None;
                }
                pending = entry.available.wait(pending).unwrap();
            }
        }
    }

    /// Clones share the same hub entry (they accept from one queue; the
    /// test-fixture face).
    #[derive(Clone)]
    pub struct MemoryListener {
        hub: Arc<MemoryHub>,
        path: String,
    }

    impl ControlListener for MemoryListener {
        fn accept(&self) -> io::Result<Box<dyn ControlSocket>> {
            self.hub
                .take_pending(&self.path)
                .map(|socket| Box::new(socket) as Box<dyn ControlSocket>)
                .ok_or_else(|| io::Error::other("listener closed"))
        }

        fn path(&self) -> &str {
            &self.path
        }

        fn close(&self) {
            if let Some(entry) = self.hub.entries.lock().unwrap().get(&self.path) {
                entry
                    .bound
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                let mut pending = entry.pending.lock().unwrap();
                pending.clear();
                entry.available.notify_all();
            }
        }
    }

    pub struct MemoryConnector {
        hub: Arc<MemoryHub>,
    }

    impl MemoryConnector {
        pub fn new(hub: Arc<MemoryHub>) -> Self {
            MemoryConnector { hub }
        }
    }

    impl ControlConnector for MemoryConnector {
        fn connect(&self, path: &str) -> io::Result<Box<dyn ControlSocket>> {
            self.hub.connect(path)
        }
    }
}

pub use memory::{MemoryConnector, MemoryHub, MemoryListener};
