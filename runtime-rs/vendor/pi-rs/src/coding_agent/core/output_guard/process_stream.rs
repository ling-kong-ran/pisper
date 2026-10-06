//! Safe, FIFO process IO adapter. Each stream has one blocking writer thread;
//! callback completion includes flushing std's userspace buffer. The generic
//! guard supports arbitrary stream booleans/codecs; this UTF-8 native boundary
//! conservatively returns false until callback completion (not a reproduction
//! of Node's platform-dependent stream high-water mark). Modes await callbacks.
#[cfg(test)]
use super::WriteResult;
use super::{OutputChunk, OutputError, OutputStream, WriteCallback};
use serde_json::Value;
use std::io::{self, Write};
use std::sync::{mpsc, Arc};

struct Request {
    bytes: Vec<u8>,
    callback: Option<WriteCallback>,
}
pub(super) struct ProcessStream {
    sender: mpsc::Sender<Request>,
}
impl ProcessStream {
    pub(super) fn stdout() -> Self {
        Self::new(false)
    }
    pub(super) fn stderr() -> Self {
        Self::new(true)
    }
    fn new(stderr: bool) -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        std::thread::spawn(move || {
            for request in receiver {
                let result = if stderr {
                    let mut stream = io::stderr().lock();
                    stream
                        .write_all(&request.bytes)
                        .and_then(|()| stream.flush())
                } else {
                    let mut stream = io::stdout().lock();
                    stream
                        .write_all(&request.bytes)
                        .and_then(|()| stream.flush())
                }
                .map_err(from_io);
                if let Some(callback) = request.callback {
                    callback(result);
                }
            }
        });
        Self { sender }
    }
}
impl OutputStream for ProcessStream {
    fn write(
        &self,
        chunk: OutputChunk,
        encoding: Option<String>,
        callback: Option<WriteCallback>,
    ) -> Result<bool, Arc<OutputError>> {
        if encoding
            .as_deref()
            .is_some_and(|s| !s.eq_ignore_ascii_case("utf8") && !s.eq_ignore_ascii_case("utf-8"))
        {
            return Err(OutputError::new(
                "Native output stream only accepts UTF-8 encoding",
                Some(Value::String("ERR_UNKNOWN_ENCODING".into())),
            ));
        }
        let bytes = match chunk {
            OutputChunk::Text(text) => text.into_bytes(),
            OutputChunk::Buffer(bytes) | OutputChunk::Uint8Array(bytes) => bytes,
        };
        self.sender.send(Request { bytes, callback }).map_err(|_| {
            OutputError::new("Output stream closed", Some(Value::String("EPIPE".into())))
        })?;
        Ok(false)
    }
}
fn from_io(error: io::Error) -> Arc<OutputError> {
    OutputError::new(
        error.to_string(),
        retry_code(&error).map(|code| Value::String(code.into())),
    )
}
fn retry_code(error: &io::Error) -> Option<&'static str> {
    #[cfg(unix)]
    if let Some(code) = error.raw_os_error() {
        if code == libc::ENOBUFS {
            return Some("ENOBUFS");
        }
        if [libc::EAGAIN, libc::EWOULDBLOCK].contains(&code) {
            return Some("EAGAIN");
        }
    }
    // libuv v1.51.0 src/win/error.c (the captured Node oracle's version):
    // WSAENOBUFS -> ENOBUFS; WSAEWOULDBLOCK / ERROR_NO_DATA -> EAGAIN.
    #[cfg(windows)]
    match error.raw_os_error() {
        Some(10055) => return Some("ENOBUFS"),
        Some(10035 | 232) => return Some("EAGAIN"),
        _ => {}
    }
    (error.kind() == io::ErrorKind::WouldBlock).then_some("EAGAIN")
}
#[cfg(test)]
pub(super) fn classify_for_test(error: io::Error) -> WriteResult {
    Err(from_io(error))
}
