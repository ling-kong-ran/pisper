//! Native speech domain. All model contracts originate in the shared release catalog.
#[path = "native_speech/asr_abi.rs"]
mod asr_abi;
#[path = "native_speech/catalog.rs"]
pub mod catalog;
#[path = "native_speech/downloads.rs"]
pub mod downloads;
#[path = "native_speech/error.rs"]
pub mod error;
#[path = "native_speech/native.rs"]
pub mod native;
#[path = "native_speech/storage.rs"]
pub mod storage;
#[path = "native_speech/terms.rs"]
pub mod terms;
#[cfg(test)]
#[path = "native_speech/tests.rs"]
mod tests;
#[path = "native_speech/tts_abi.rs"]
mod tts_abi;
#[path = "native_speech/worker.rs"]
pub mod worker;
pub use worker::worker_main;
#[path = "native_speech/service.rs"]
pub mod service;
pub use service::SpeechService;
#[path = "native_speech/http.rs"]
mod http;
pub use http::{router, SpeechSessionResolver};
