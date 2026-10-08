//! Port of `packages/telemetry` (`index.ts` 357 lines, `noop.ts` 20 lines,
//! `memory.ts` 219 lines): the backend-neutral telemetry vocabulary the
//! harness subtree consumes.
//!
//! Upstream splits this package into a type-only surface (attribute/status
//! context types and the large compile-time schema inference machinery,
//! `index.ts:1-69` and `index.ts:76-354`) and a small runtime core (the
//! `TelemetryContext.startSpan` contract, the frozen no-op context, the
//! in-memory recording backend, and `defineTelemetrySchema`/`createTypedSpanStarter`).
//! The type-level inference machinery has no runtime behavior and maps to
//! Rust's ownership and generics, so only the runtime core is ported here;
//! the schema *data* model (ordered attribute/spans tables) lives in
//! [`schema`] because `packages/agent/src/harness/telemetry.ts` exports two
//! schema constants whose serialized shape is a compatibility surface.
//!
//! Substitutions (all disclosed upstream-shape-preserving):
//! - `TelemetryContext.startSpan<T>` is generic in TypeScript; a trait-object
//!   surface cannot carry generic methods, so the erased transport uses
//!   `Box<dyn Any + Send>` ([`Captured`]) and the public wrapper
//!   [`TelemetryContext::start_span`] downcasts once. Callbacks return
//!   futures, which reproduces upstream's "sync throw rejects" via the
//!   future's `Err` path.
//! - Upstream attributes are objects whose values may be `undefined`;
//!   `copyAttributes` drops those entries. `undefined` is unrepresentable in
//!   [`SpanAttributes`], so call sites omit the key — the observable
//!   post-copy record is identical.
//! - Attribute maps preserve insertion order (`Vec<(String, AttributeValue)>`)
//!   with upstream object-assignment merge semantics: an existing key is
//!   replaced in place, a new key is appended.
//! - `automaticErrorStatus` inspects `error.name`/`error.message` of the
//!   thrown JS `Error`. Rust errors carry no name, so the recorded name is
//!   the fixed `"Error"` and the message is `error.to_string()` — matching
//!   upstream for plain `new Error(message)` throws, which is what the
//!   ported call sites and oracles exercise.
//!
//! The in-memory backend ([`memory::InMemoryTelemetryContext`]) is the
//! reference recorder the harness tests and oracles use; span ids start at 1
//! and end sequences start at 1, exactly as upstream.

pub mod memory;
pub mod schema;

use anyhow::anyhow;
use futures::future::BoxFuture;
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::any::Any;
use std::sync::Arc;

/// Erased span-callback result: the typed value the callback produced.
pub type Captured = Box<dyn Any + Send>;

/// Erased span body: receives the span handle, returns the callback future.
pub type ErasedSpanCallback =
    Box<dyn FnOnce(Arc<dyn TelemetrySpanT>) -> BoxFuture<'static, anyhow::Result<Captured>> + Send>;

/// Upstream `AttributeValue` (`index.ts:1`). Numbers use `serde_json::Number`
/// so integer-valued attributes serialize without a trailing `.0`, matching
/// JavaScript's number formatting for the values call sites produce.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum AttributeValue {
    Str(String),
    Num(serde_json::Number),
    Bool(bool),
    StrArray(Vec<String>),
    NumArray(Vec<serde_json::Number>),
    BoolArray(Vec<bool>),
}

/// Upstream `SpanAttributes` (`index.ts:3-5`): insertion-ordered name/value
/// pairs. `undefined`-valued entries are unrepresentable (module docs).
pub type SpanAttributes = Vec<(String, AttributeValue)>;

/// Upstream `SpanOptions` (`index.ts:7-10`).
#[derive(Debug, Clone, Default)]
pub struct SpanOptions {
    pub name: String,
    pub attributes: SpanAttributes,
}

impl SpanOptions {
    /// Upstream `{ name, attributes? }` object construction.
    pub fn new(name: impl Into<String>, attributes: SpanAttributes) -> Self {
        Self {
            name: name.into(),
            attributes,
        }
    }
}

/// Upstream `SpanStatus["error"]` record (`index.ts:12`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpanError {
    pub name: String,
    pub message: String,
}

/// Upstream `SpanStatus` (`index.ts:12`): `{status:"ok"}` or
/// `{status:"error", error?: {name, message}}` — the `error` key is absent
/// when no error detail is carried.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanStatus {
    Ok,
    Error { error: Option<SpanError> },
}

impl Serialize for SpanStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            SpanStatus::Ok => {
                map.serialize_entry("status", "ok")?;
            }
            SpanStatus::Error { error } => {
                map.serialize_entry("status", "error")?;
                if let Some(error) = error {
                    map.serialize_entry("error", error)?;
                }
            }
        }
        map.end()
    }
}

/// Upstream `TelemetrySpan` runtime surface (`index.ts:18-22`): an active
/// span is also a telemetry context (children start from it). The typed
/// schema-aware methods are enforced at call sites by the harness schema
/// wrappers; the transport carries the raw vocabulary.
pub trait TelemetrySpanT: Send + Sync + 'static {
    fn add_event(&self, name: &str, attributes: SpanAttributes);
    fn set_attributes(&self, attributes: SpanAttributes);
    fn set_status(&self, status: SpanStatus);
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>>;
}

/// Upstream `TelemetryContext` runtime surface (`index.ts:14-16`).
pub trait RawTelemetryContext: Send + Sync + 'static {
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>>;
}

/// Shared no-op span (`noop.ts:11-16`): every recording method is a no-op and
/// child spans are also no-ops; the callback still runs and its `Err`
/// propagates (upstream wraps sync throws into the rejected promise, which is
/// the future's `Err` here).
#[derive(Debug, Default)]
struct NoopSpan;

impl TelemetrySpanT for NoopSpan {
    fn add_event(&self, _name: &str, _attributes: SpanAttributes) {}
    fn set_attributes(&self, _attributes: SpanAttributes) {}
    fn set_status(&self, _status: SpanStatus) {}
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>> {
        noop_start(options, callback)
    }
}

fn noop_start(
    _options: SpanOptions,
    callback: ErasedSpanCallback,
) -> BoxFuture<'static, anyhow::Result<Captured>> {
    Box::pin(async move { callback(Arc::new(NoopSpan)).await })
}

#[derive(Debug, Default)]
struct NoopTelemetry;

impl RawTelemetryContext for NoopTelemetry {
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>> {
        noop_start(options, callback)
    }
}

/// An active span viewed as a telemetry context (upstream `TelemetrySpan
/// extends TelemetryContext`).
struct SpanAsContext(Arc<dyn TelemetrySpanT>);

impl RawTelemetryContext for SpanAsContext {
    fn start_span_erased(
        &self,
        options: SpanOptions,
        callback: ErasedSpanCallback,
    ) -> BoxFuture<'static, anyhow::Result<Captured>> {
        self.0.start_span_erased(options, callback)
    }
}

/// Cloneable handle over the erased backend (`NOOP_TELEMETRY_CONTEXT`,
/// `InMemoryTelemetryContext`, or an active span used as a parent).
#[derive(Clone)]
pub struct TelemetryContext {
    inner: Arc<dyn RawTelemetryContext>,
}

impl TelemetryContext {
    /// Upstream `NOOP_TELEMETRY_CONTEXT` (`noop.ts:20`): the shared context
    /// used when an application does not provide one.
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(NoopTelemetry),
        }
    }

    /// View an active span as a parent context (upstream spans are contexts).
    pub fn from_span(span: Arc<dyn TelemetrySpanT>) -> Self {
        Self {
            inner: Arc::new(SpanAsContext(span)),
        }
    }

    /// Upstream `startSpan<T>(options, callback)`; the typed result is
    /// carried through the erased transport via `Box<dyn Any + Send>`.
    pub fn start_span<T, F>(
        &self,
        options: SpanOptions,
        callback: F,
    ) -> BoxFuture<'static, anyhow::Result<T>>
    where
        F: FnOnce(Arc<dyn TelemetrySpanT>) -> BoxFuture<'static, anyhow::Result<T>>
            + Send
            + 'static,
        T: Send + 'static,
    {
        let erased: ErasedSpanCallback = Box::new(move |span| {
            Box::pin(async move {
                let value = callback(span).await?;
                Ok(Box::new(value) as Captured)
            })
        });
        let fut = self.inner.start_span_erased(options, erased);
        Box::pin(async move {
            let captured = fut.await?;
            captured
                .downcast::<T>()
                .map(|value| *value)
                .map_err(|_| anyhow!("telemetry span result type mismatch"))
        })
    }
}

/// Start a typed child span from an active span handle (upstream
/// `span.startSpan<T>(...)` — spans are contexts).
pub fn start_child_span<T, F>(
    span: &Arc<dyn TelemetrySpanT>,
    options: SpanOptions,
    callback: F,
) -> BoxFuture<'static, anyhow::Result<T>>
where
    F: FnOnce(Arc<dyn TelemetrySpanT>) -> BoxFuture<'static, anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    TelemetryContext::from_span(Arc::clone(span)).start_span(options, callback)
}

/// Upstream `TypedSpanStarter` (`index.ts:318-322`): a per-span overload set
/// bound to one explicit parent context and one or more schemas. The schema
/// values are used only for typing; no runtime validation is performed
/// (`index.ts:345-348`), so the port carries no schema data.
#[derive(Clone)]
pub struct TypedSpanStarter {
    telemetry: TelemetryContext,
}

impl TypedSpanStarter {
    /// Upstream `createTypedSpanStarter(telemetryContext, schemas)`
    /// (`index.ts:349-354`): binds an explicit parent context to the span
    /// vocabulary; the schemas parameter is identity-only.
    pub fn new(
        telemetry: TelemetryContext,
        _schemas: &[&'static schema::TelemetrySchemaDefinition],
    ) -> Self {
        Self { telemetry }
    }

    /// Upstream the starter is called as `(name, attributes, callback)` where
    /// the callback receives the span and a child starter bound to it.
    pub fn start<T, F>(
        &self,
        options: SpanOptions,
        callback: F,
    ) -> BoxFuture<'static, anyhow::Result<T>>
    where
        F: FnOnce(
                Arc<dyn TelemetrySpanT>,
                TypedSpanStarter,
            ) -> BoxFuture<'static, anyhow::Result<T>>
            + Send
            + 'static,
        T: Send + 'static,
    {
        self.telemetry.start_span(options, move |span| {
            let child = TypedSpanStarter {
                telemetry: TelemetryContext::from_span(Arc::clone(&span)),
            };
            callback(span, child)
        })
    }
}

/// Serialize insertion-ordered attribute pairs as a JSON object. Upstream
/// objects iterate in insertion order; `serde_json::Value` maps would sort,
/// so the schema and record serializers stream entries instead.
pub(crate) fn serialize_attribute_map<S: Serializer>(
    attributes: &[(String, AttributeValue)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(attributes.len()))?;
    for (name, value) in attributes {
        map.serialize_entry(name, value)?;
    }
    map.end()
}

/// Upstream object-assignment merge (`memory.ts:63-69`): an existing key is
/// replaced in place, a new key is appended.
pub(crate) fn merge_attributes(current: &mut SpanAttributes, attributes: SpanAttributes) {
    for (name, value) in attributes {
        match current.iter_mut().find(|(existing, _)| *existing == name) {
            Some(slot) => slot.1 = value,
            None => current.push((name, value)),
        }
    }
}

#[cfg(test)]
mod tests;
