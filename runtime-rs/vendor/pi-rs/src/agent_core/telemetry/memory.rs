//! Port of `packages/telemetry/src/memory.ts` (219 lines): the backend-neutral
//! reference recorder used by harness tests and oracles.
//!
//! Faithful semantics (upstream line references in comments):
//! - span ids start at 1 (`nextSpanId`), end sequences start at 1
//!   (`nextEndSequence`), assigned at settle time in settle order
//!   (`memory.ts:194-197`, `memory.ts:89-99`);
//! - a span started from an already-settled parent delegates to the no-op
//!   context — nothing is recorded (`memory.ts:126`);
//! - `addEvent`/`setAttributes`/`setStatus` are ignored once settled
//!   (`memory.ts:141-165`);
//! - `setAttributes` merges with object-assignment semantics (existing key
//!   replaced in place, new key appended, `memory.ts:63-69`);
//! - a callback failure without an explicit status records
//!   `automaticErrorStatus` (`memory.ts:78-87`): upstream inspects
//!   `error.name`/`error.message`; Rust errors carry no name, so the port
//!   records the fixed name `"Error"` with `error.to_string()` as the
//!   message (module docs of the parent module);
//! - `getSpans` returns detached snapshots in span-start order, omitting
//!   `endSequence` for unsettled spans (`memory.ts:204-218`).

use super::{
    merge_attributes, Captured, ErasedSpanCallback, RawTelemetryContext, SpanAttributes, SpanError,
    SpanOptions, SpanStatus, TelemetrySpanT,
};
use futures::future::BoxFuture;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use std::sync::{Arc, Mutex};

/// Upstream `RecordedTelemetryEvent` (`memory.ts:11-14`).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTelemetryEvent {
    pub name: String,
    pub attributes: SpanAttributes,
}

impl Serialize for RecordedTelemetryEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("attributes", &AttributesRef(&self.attributes))?;
        map.end()
    }
}

/// Upstream `RecordedTelemetrySpan` (`memory.ts:16-25`): the detached
/// snapshot shape; `end_sequence` serializes only when settled.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTelemetrySpan {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub attributes: SpanAttributes,
    pub events: Vec<RecordedTelemetryEvent>,
    pub status: SpanStatus,
    pub settled: bool,
    pub end_sequence: Option<i64>,
}

struct AttributesRef<'a>(&'a SpanAttributes);

impl Serialize for AttributesRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        super::serialize_attribute_map(self.0, serializer)
    }
}

impl Serialize for RecordedTelemetrySpan {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map =
            serializer.serialize_map(Some(if self.end_sequence.is_some() { 8 } else { 7 }))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("parentId", &self.parent_id)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("attributes", &AttributesRef(&self.attributes))?;
        map.serialize_entry("events", &EventsRef(&self.events))?;
        map.serialize_entry("status", &self.status)?;
        map.serialize_entry("settled", &self.settled)?;
        if let Some(end_sequence) = self.end_sequence {
            map.serialize_entry("endSequence", &end_sequence)?;
        }
        map.end()
    }
}

struct EventsRef<'a>(&'a [RecordedTelemetryEvent]);

impl Serialize for EventsRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for event in self.0 {
            seq.serialize_element(event)?;
        }
        seq.end()
    }
}

struct MutableSpan {
    id: i64,
    parent_id: Option<i64>,
    name: String,
    attributes: SpanAttributes,
    events: Vec<RecordedTelemetryEvent>,
    status: SpanStatus,
    explicit_status: bool,
    settled: bool,
    end_sequence: Option<i64>,
}

struct MemoryState {
    spans: Vec<Arc<Mutex<MutableSpan>>>,
    next_span_id: i64,
    next_end_sequence: i64,
}

/// Upstream `InMemoryTelemetryContext` (`memory.ts:192-219`). Clone shares
/// the recording scope, mirroring the shared-state instance semantics.
#[derive(Clone)]
pub struct InMemoryTelemetryContext {
    state: Arc<Mutex<MemoryState>>,
}

impl InMemoryTelemetryContext {
    /// Create a fresh recording scope (`memory.ts:188-191` doc: create a new
    /// instance to isolate tests or independent recording scopes).
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState {
                spans: Vec::new(),
                next_span_id: 1,
                next_end_sequence: 1,
            })),
        }
    }

    /// Upstream `getSpans` (`memory.ts:204-218`): detached snapshots in
    /// span-start order.
    pub fn get_spans(&self) -> Vec<RecordedTelemetrySpan> {
        let state = self.state.lock().unwrap();
        state
            .spans
            .iter()
            .map(|span| {
                let span = span.lock().unwrap();
                RecordedTelemetrySpan {
                    id: span.id,
                    parent_id: span.parent_id,
                    name: span.name.clone(),
                    attributes: span.attributes.clone(),
                    events: span.events.clone(),
                    status: span.status.clone(),
                    settled: span.settled,
                    end_sequence: span.end_sequence,
                }
            })
            .collect()
    }
}

impl RawTelemetryContext for InMemoryTelemetryContext {
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>> {
        start_memory_span(Arc::clone(&self.state), None, options, callback)
    }
}

impl Default for InMemoryTelemetryContext {
    fn default() -> Self {
        Self::new()
    }
}

impl From<InMemoryTelemetryContext> for super::TelemetryContext {
    fn from(value: InMemoryTelemetryContext) -> Self {
        super::TelemetryContext {
            inner: Arc::new(value),
        }
    }
}

fn automatic_error_status(message: String) -> SpanStatus {
    SpanStatus::Error {
        error: Some(SpanError {
            name: "Error".to_string(),
            message,
        }),
    }
}

/// Upstream `startInMemorySpan` (`memory.ts:120-186`).
fn start_memory_span(
    state: Arc<Mutex<MemoryState>>,
    parent: Option<Arc<Mutex<MutableSpan>>>,
    options: SpanOptions,
    callback: ErasedSpanCallback,
) -> BoxFuture<'static, anyhow::Result<Captured>> {
    if let Some(parent) = &parent {
        // `memory.ts:126`: a child of a settled span records nothing.
        if parent.lock().unwrap().settled {
            return super::noop_start(options, callback);
        }
    }

    let span = {
        let mut state = state.lock().unwrap();
        let id = state.next_span_id;
        state.next_span_id += 1;
        let parent_id = parent.as_ref().map(|parent| parent.lock().unwrap().id);
        let span = Arc::new(Mutex::new(MutableSpan {
            id,
            parent_id,
            name: options.name,
            attributes: options.attributes,
            events: Vec::new(),
            status: SpanStatus::Ok,
            explicit_status: false,
            settled: false,
            end_sequence: None,
        }));
        state.spans.push(Arc::clone(&span));
        span
    };

    let handle: Arc<dyn TelemetrySpanT> = Arc::new(MemorySpanHandle {
        state: Arc::clone(&state),
        inner: Arc::clone(&span),
    });

    Box::pin(async move {
        let result = callback(handle).await;
        // `memory.ts:176-185`: settle with the automatic error status when the
        // body failed without an explicit status; success leaves the status.
        {
            // Lock order: state, then span (same order as `get_spans`).
            let mut state = state.lock().unwrap();
            let mut span = span.lock().unwrap();
            if !span.settled {
                if let Err(error) = &result {
                    if !span.explicit_status {
                        span.status = automatic_error_status(error.to_string());
                    }
                }
                span.settled = true;
                span.end_sequence = Some(state.next_end_sequence);
                state.next_end_sequence += 1;
            }
        }
        result
    })
}

struct MemorySpanHandle {
    state: Arc<Mutex<MemoryState>>,
    inner: Arc<Mutex<MutableSpan>>,
}

impl TelemetrySpanT for MemorySpanHandle {
    fn add_event(&self, name: &str, attributes: SpanAttributes) {
        // `memory.ts:141-148`: ignored once settled; recording is passive.
        let mut span = self.inner.lock().unwrap();
        if span.settled {
            return;
        }
        span.events.push(RecordedTelemetryEvent {
            name: name.to_string(),
            attributes,
        });
    }

    fn set_attributes(&self, attributes: SpanAttributes) {
        // `memory.ts:149-156`.
        let mut span = self.inner.lock().unwrap();
        if span.settled {
            return;
        }
        merge_attributes(&mut span.attributes, attributes);
    }

    fn set_status(&self, status: SpanStatus) {
        // `memory.ts:157-165`: an explicit status suppresses the automatic one.
        let mut span = self.inner.lock().unwrap();
        if span.settled {
            return;
        }
        span.status = status;
        span.explicit_status = true;
    }

    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>> {
        start_memory_span(
            Arc::clone(&self.state),
            Some(Arc::clone(&self.inner)),
            options,
            callback,
        )
    }
}
