//! Chord public API surface (deterministic core). Port of the subset of
//! `packages/chord/src/api.ts` (upstream sha256
//! `2ad07e4621229bdc3cfb7d917ed3f098d36fa84d3bcbcad7c2b50edd973ad087`) that
//! this slice covers:
//!
//! - [`define_service`] — `defineService` (`api.ts:72-84`), including the
//!   empty-ID and reserved `$chord.` namespace checks and the `local` flag.
//! - [`replicated_state`] — `replicatedState(initial)`
//!   (`api.ts:91-109`), and [`replicated_state_from_source`] — the
//!   `replicatedState(source, options)` overload
//!   (`api.ts:110-112, 115-119`), split into a named constructor because
//!   Rust has no duck-typed overloads.
//!
//! Deferred with the facet seam (M6 report, S2): `createFacetHost`,
//! `createStaticFacetLoader`, `combineFacetLoaders` (they wrap
//! `facets/host.ts` and `facets/loader.ts`), and `createRemoteServiceBinding`
//! (wraps `services/consumer.ts`). [`crate::chord::services::provider`]
//! covers the provider side of the binding surface.

use std::sync::Arc;

use crate::chord::services::state::{
    attach_replicated_state_source, AttachedReplicatedState, MutableReplicatedState,
};
use crate::chord::types::{
    JsonValue, ReplicatedStateSource, ReplicatedStateSourceOptions, Service,
};

/// `defineService(id)` (`api.ts:72-84`): remotable by default.
pub fn define_service(id: &str) -> Result<Service, crate::chord::services::errors::ChordError> {
    define_service_with(id, false)
}

/// `defineService(id, { local: true })` (`api.ts:72-84`).
pub fn define_service_local(
    id: &str,
) -> Result<Service, crate::chord::services::errors::ChordError> {
    define_service_with(id, true)
}

fn define_service_with(
    id: &str,
    local: bool,
) -> Result<Service, crate::chord::services::errors::ChordError> {
    if id.is_empty() {
        return Err(crate::chord::services::errors::ChordError::Type(
            "Service ID must not be empty".to_owned(),
        ));
    }
    // TODO(upstream): check if the reserved namespace should be part of Chord.
    if id.starts_with("$chord.") {
        return Err(crate::chord::services::errors::ChordError::Type(
            "Service IDs beginning with $chord. are reserved".to_owned(),
        ));
    }
    Ok(Service {
        id: id.to_owned(),
        local,
    })
}

/// `replicatedState(initial)` (`api.ts:96-99`): create authoritative state
/// by taking immutable ownership of an alias-free strict-JSON root. The
/// caller must not mutate `initial` after this call.
pub fn replicated_state(initial: JsonValue) -> Arc<MutableReplicatedState> {
    MutableReplicatedState::new(initial)
}

/// The `replicatedState(source, options)` overload (`api.ts:91-95,
/// 110-112`): attach a publication-only state to one authoritative
/// immutable source stream.
pub fn replicated_state_from_source(
    source: Arc<dyn ReplicatedStateSource>,
    options: ReplicatedStateSourceOptions,
) -> Result<Arc<AttachedReplicatedState>, crate::chord::services::errors::ChordError> {
    attach_replicated_state_source(source, options)
}
