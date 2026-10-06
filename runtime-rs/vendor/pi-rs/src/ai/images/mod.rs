//! The image-generation side (upstream surface after #9948 unified the model
//! infrastructure): the image data types live in [`crate::ai::types::images`]
//! and the [`ImageModel`](crate::ai::types::model::ImageModel) catalog entry
//! in [`crate::ai::types::model`]; this module ports the runtime half —
//!
//! - [`registry`] — the images API provider registry, the free
//!   [`generate_images`](registry::generate_images) entry point, and the
//!   static image-model catalog compat reads (`image-models.ts`, now reading
//!   the unified generated catalog), plus the OpenRouter image-model parser
//!   (upstream `images-api-registry.ts`, `images.ts`,
//!   `scripts/generate-image-models.ts`).
//! - [`openrouter_images`] — the `openrouter-images` API implementation
//!   (upstream `api/openrouter-images.ts`).
//!
//! Upstream **deleted** `images-models.ts` (the `ImagesModels` collection —
//! auth-resolving generation moved into `Models.generateImages`) and
//! `providers/openrouter-images.ts` (the standalone image provider factory —
//! OpenRouter now serves its image models through the chat provider's
//! `images` map, see `models::providers::openrouter_provider`); the port
//! removed the corresponding modules with them. `images.ts` pulls the
//! built-in API registrations in through an import side effect
//! (`providers/images/register-builtins.ts`); the port registers them lazily
//! at first [`registry::generate_images`] call (Rust has no import side
//! effects).

pub mod openrouter_images;
pub mod registry;

pub use registry::{
    ensure_builtin_images_apis_registered, generate_images, get_image_model, get_image_models,
    get_image_providers, get_images_api_provider, parse_open_router_image_models,
    register_images_api_provider, ImagesApiFn, OPENROUTER_BASE_URL,
};
