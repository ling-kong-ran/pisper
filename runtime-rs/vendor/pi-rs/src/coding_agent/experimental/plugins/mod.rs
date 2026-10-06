//! Port of upstream `experimental/plugins/` — `bundled.ts`
//! (sha256 aeb9e63fcd487c0230e1f54ec033c6a1f6fef283802250945c05159d03d65244)
//! and `package.ts`
//! (sha256 ea626c8a56aa2bf81b2b67646e60a9f1f0467633e4f8cf9836ba948dd2b0702f).
//!
//! Ported: the presentation facet-bundle data envelope
//! (`createPresentationFacetData` / `createPresentationFacetLoaders`
//! validation with exact error strings), the plugin package profile
//! persistence protocol (version/sessionPath/packagePaths document shape,
//! `allowEmpty` server-vs-session distinction, exact JSON serialization and
//! validation error strings), `normalizePluginPackagePaths`, the sha256-derived
//! `session-plugin-packages-<serverId>-<hash>.json` profile path and the
//! `pluginBuildDirectoryName` label scheme, and the serialized per-package
//! build tail of [`ServerPluginPackage`].
//!
//! D9 seam (disclosed in this module's docs): the chord bundler (`bundleFacetPackage`,
//! `readFacetBundleArtifact`) is embedder-owned; [`ServerPluginPackage::build`]
//! reports the manifest path and delegates artifact production through the
//! [`FacetBundleBuild`] seam. Facet loading (`createSessionPluginFacetLoader`)
//! is likewise embedder-owned; only the empty-manifest short circuit is
//! deterministic and ported.

pub mod bundled;
pub mod package;

#[cfg(test)]
mod tests;
