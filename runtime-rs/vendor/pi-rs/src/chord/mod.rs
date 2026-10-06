//! # chord — standalone application-composition runtime. Port of
//! `pi/packages/chord` (M6 + the delta slice). Upstream sha256
//! registrations (source of truth for this slice):
//!
//! | Upstream file | sha256 |
//! | --- | --- |
//! | `src/delta/index.ts` | `a95d221ff4ea9b48bd129904a3c4daacf9dd10182163d5d3f8e0b1c21b533b34` |
//! | `src/delta/tracker.ts` | `3be4c18b42a4fff288617d96f8547137a485b41b0f455c2c811beae5b4bc5778` |
//! | `src/delta/diff.ts` | `4d1ee245aef0967154ff7177527dcf68606cde400f6cfcfe00cedd4c1e21986e` |
//! | `src/delta/apply-immutable-trusted.ts` | `861e7f8b7f410326cfb9658c272ee0d5e6235def40c3304237e41a5a49d3ec69` |
//! | `src/delta/revision-validator.ts` | `d4d5c62b67271ecfe3029b0619a333b4d8a96a2bb6b8015fe83bffd0f7718c09` |
//! | `src/json.ts` | `8b7d633aabcf694f3827355a9d5bc305efbcdb6c276f9420509eeccfa613edf8` |
//! | `src/types.ts` | `adb3602bbf8ccdc94015b950151504e890612cdd1e836366137ceaa4f228b5f4` |
//! | `src/api.ts` | `2ad07e4621229bdc3cfb7d917ed3f098d36fa84d3bcbcad7c2b50edd973ad087` |
//! | `src/context/index.ts` | `0f0925a896cf1e506d9495d929157347639e90da343bf28d7472ea645897e7d3` |
//! | `src/services/provider.ts` | `47ca6cd540079906cff962e58a86961ee52f82ef8949fe4674e964bcfaf12100` |
//! | `src/services/wire.ts` | `dbe4011b774fbeb90535d4c89ec0ffe19aa3828b602db9cc75506fb2f39eef74` |
//! | `src/services/state.ts` | `6722c58e0816120d4f48a6d58d0d7248c3fe6dd9baf6f7cfd9181a56f2797668` |
//! | `src/services/state-codec.ts` | `a42c889ae7659ef5dfabca835b3e59fb85796f0b45f58666f8418d4e34f40834` |
//! | `src/services/state-internals.ts` | `1ff3650206497e3c0863f978b358593bacaee437d7e141b80e31327c00629cea` |
//! | `src/services/errors.ts` | `487e73b60a1c69dd132cf6f4c4ec625e346b5cecc1cb890da25a5199a43798a8` |
//!
//! ## Slice boundary (M6 chord slice)
//!
//! Ported here with oracle byte-comparison against the upstream sources run
//! under `node --experimental-strip-types`:
//!
//! - [`delta`] — the full `delta/index.ts`: ops, validators, `apply` /
//!   `applyImmutable`, the write-time tracker log (coalescing, folding,
//!   collapse-to-base), the path-interning wire codec, and the diff engine.
//! - [`json`] — `isJsonValue` (depth bound; the other JS-only guards are
//!   unrepresentable over owned JSON trees).
//! - [`types`] — the service value shapes (snapshots, updates, calls).
//! - [`services`] — provider, endpoint, replicated state (mutable + replica),
//!   wire protocol parsing, per-subscription state codecs, error taxonomy.
//! - [`api`] — `defineService` / `replicatedState` constructors.
//! - [`context`] — forwards to the already-ported
//!   `agent_core::chord_support::context` (kept compatible; not deleted).
//!
//! Formerly deferred seams, closed by the consumer-side slice (S1/S2/S3):
//!
//! - **S1 (consumer binding)**, now [`consumer`]/[`handle`]/[`instances`]:
//!   `services/consumer.ts`, `handle.ts`, `instances.ts`,
//!   `loopback.ts`, and the transport surface
//!   (`RemoteServiceTransport`/`RemoteServices`). The upstream async
//!   lifecycle state machine is flattened to the synchronous closure
//!   convention (divergence D2); readiness revisions and rebind
//!   transitions are preserved.
//! - **S2 (facets)**, now [`facets`]: `facets/host.ts`,
//!   `facets/loader.ts`, and `api.ts`'s host/loader constructors
//!   (`create_facet_host`, `create_static_facet_loader`,
//!   `combine_facet_loaders`, plus `create_remote_service_binding`).
//! - **S3 (node/bundler)**, now [`node`]/[`bundler`]: the deterministic
//!   manifest/artifact/package-metadata surface. The esbuild compile pass
//!   and the node:vm CommonJS evaluation are platform seams (divergences
//!   D7/D9), disclosed in the child module docs.
//!
//! ## Canonical serialization note (disclosed divergence D1)
//!
//! Upstream JS objects iterate in insertion order; `serde_json`'s default map
//! sorts keys (BTreeMap). The oracle comparison therefore canonicalizes both
//! sides with recursively sorted object keys (see `tests/fixtures/chord_oracle/`),
//! and oracle scenarios build multi-key objects in sorted key order so
//! diff/iteration orders coincide. Value-level semantics are unaffected:
//! upstream's own contract states object key order is not replicated
//! (`delta/README.md` "Limits").
//!
//! The [`agent_core::chord_support`] subset (context + a diff-at-flush delta
//! tracker) predates this module and stays untouched; `chord_support` can
//! later re-export [`delta`] as its full-fidelity replacement.

pub mod api;
pub mod bundler;
pub mod consumer;
pub mod context;
pub mod delta;
pub mod facets;
pub mod handle;
pub mod instances;
pub mod json;
pub mod node;
pub mod services;
pub mod types;

/// Upstream `JsonValue` (`packages/chord/src/types.ts:21`).
pub use types::JsonValue;
