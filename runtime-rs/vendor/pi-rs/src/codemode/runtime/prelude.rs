//! Port of upstream `codemode/src/runtime/prelude-source.ts` — the JavaScript
//! evaluated inside the VM before the script runs.
//!
//! [`PRELUDE_SOURCE`] is **byte-exact**: it is embedded from
//! `assets/codemode/prelude.js`, a byte-identical copy of the ground-truth
//! fixture `tests/fixtures/codemode_oracle/src/runtime/prelude.js` (generated
//! from the verbatim upstream `prelude-source.ts` — its interpolation of
//! `MAX_STORE_VALUE_CHARS` / `MAX_STORE_TOTAL_CHARS` /
//! `JSON.stringify(IMAGE_HELPER_EXPECTS)` already applied — by
//! `tests/fixtures/codemode_oracle/gen_prelude.tmp.mjs`). The copy lives under
//! `assets/` because the crate must compile from the published package, which
//! excludes the ~69 MB fixture tree; the SHA-256 test below pins the asset to
//! the fixture's hash, so the two cannot drift. The upstream doc-comment of
//! the prelude applies unchanged: it keeps the host bridge in a closure,
//! builds `tools`, `ALL_TOOLS`, the output helpers (`text`, `image`, `exit`,
//! `console`), and `store`/`load`, evaluates to
//! `(bridge, toolsJson, globalsJson, storeJson) => { settle, run, stalled }`,
//! and `stalled()` reports scripts waiting on promises nothing can resume.

/// Upstream `MAX_STORE_VALUE_CHARS` (256 KiB characters of JSON per value).
pub const MAX_STORE_VALUE_CHARS: usize = 256 * 1024;
/// Upstream `MAX_STORE_TOTAL_CHARS` (1 MiB characters of JSON in total).
pub const MAX_STORE_TOTAL_CHARS: usize = 1024 * 1024;
/// Upstream `MAX_OUTPUT_CHARS` (16 MiB characters of text and base64 image
/// data one script may produce).
pub const MAX_OUTPUT_CHARS: usize = 16 * 1024 * 1024;
/// Upstream `MAX_OUTPUT_ITEMS` (100,000 `text()`, `image()`, and `console`
/// output items one script may produce).
pub const MAX_OUTPUT_ITEMS: usize = 100_000;

pub const PRELUDE_SOURCE: &str = include_str!("../../../assets/codemode/prelude.js");

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The fixture `prelude.js` SHA-256, pinned when the v1.0.0 prelude was
    /// embedded. Guards the `assets/` copy against drift from the ground-truth
    /// fixture (the published package ships the asset, not the fixture tree).
    const PRELUDE_SOURCE_SHA256: &str =
        "c8c292ac0bc913654384ae12ecd759d6de89862acab0ce912f2e733878749880";

    #[test]
    fn prelude_mentions_the_expected_helpers() {
        assert!(PRELUDE_SOURCE.contains("function store(key, value)"));
        assert!(PRELUDE_SOURCE.contains("settle(id, ok, payload)"));
        assert!(PRELUDE_SOURCE.contains(&MAX_STORE_VALUE_CHARS.to_string()));
        assert!(PRELUDE_SOURCE.contains(&MAX_STORE_TOTAL_CHARS.to_string()));
        assert!(PRELUDE_SOURCE.contains("image expects a non-empty image URL string"));
    }

    #[test]
    fn prelude_source_is_pinned_to_the_fixture_hash() {
        let digest = Sha256::digest(PRELUDE_SOURCE.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, PRELUDE_SOURCE_SHA256);
    }
}
