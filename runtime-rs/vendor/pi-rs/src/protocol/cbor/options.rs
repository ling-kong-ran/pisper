//! CBOR limits and error types. Port of `packages/protocol/src/cbor/options.ts`.

use std::fmt;

/// `2^32`, the base separating 32-bit and 64-bit CBOR arguments.
pub const UINT32_BASE: u64 = 0x1_0000_0000;

/// `0xffff_ffff`, the largest 32-bit CBOR argument.
pub const MAX_UINT32: u32 = u32::MAX;

const MAX_CONFIGURED_DEPTH: u64 = 512;

/// Safe defaults for untrusted protocol payloads.
pub const DEFAULT_MAX_CBOR_BYTE_LENGTH: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_CBOR_CONTAINER_LENGTH: u64 = 1_000_000;
pub const DEFAULT_MAX_CBOR_DEPTH: u64 = 64;

/// Largest exact JavaScript integer (`2^53 - 1`); integers outside this range
/// are rejected by both codec directions.
pub const MAX_SAFE_INTEGER: i128 = 9007199254740991;

/// Caller-tunable limits (upstream `CborOptions`). The fields are `u64`, so
/// upstream `RangeError` cases for non-integer values are type-level
/// impossible (divergence D4); representable overflows are still validated by
/// [`resolve_options`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CborOptions {
    /// Maximum encoded input/output bytes and maximum byte/text string length.
    pub max_byte_length: Option<u64>,
    /// Maximum number of elements in an array or entries in a map.
    pub max_container_length: Option<u64>,
    /// Maximum recursive item depth.
    pub max_depth: Option<u64>,
}

impl CborOptions {
    pub fn new() -> CborOptions {
        CborOptions::default()
    }

    pub fn with_max_byte_length(mut self, value: u64) -> CborOptions {
        self.max_byte_length = Some(value);
        self
    }

    pub fn with_max_container_length(mut self, value: u64) -> CborOptions {
        self.max_container_length = Some(value);
        self
    }

    pub fn with_max_depth(mut self, value: u64) -> CborOptions {
        self.max_depth = Some(value);
        self
    }
}

/// Fully resolved limits (upstream `ResolvedCborOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedCborOptions {
    pub max_byte_length: u64,
    pub max_container_length: u64,
    pub max_depth: u64,
}

/// Port of upstream `CborError`: encoding or decoding failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborError {
    message: String,
}

impl CborError {
    pub fn new(message: impl Into<String>) -> CborError {
        CborError {
            message: message.into(),
        }
    }

    /// The exact upstream `error.message` text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CborError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CborError {}

/// Port of the JS `RangeError` thrown by limit validation (upstream
/// `resolveLimit` / `resolveMaxFrameLength`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeError {
    message: String,
}

impl RangeError {
    pub fn new(message: impl Into<String>) -> RangeError {
        RangeError {
            message: message.into(),
        }
    }

    /// The exact upstream `error.message` text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RangeError {}

/// Upstream CBOR entry points throw either `CborError` or `RangeError`; the
/// port keeps the distinction. [`CborFailure::message`] matches upstream
/// `error.message` either way, which is all the codec wraps into its own
/// errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborFailure {
    Cbor(CborError),
    Range(RangeError),
}

impl CborFailure {
    pub fn message(&self) -> &str {
        match self {
            CborFailure::Cbor(error) => error.message(),
            CborFailure::Range(error) => error.message(),
        }
    }
}

impl From<CborError> for CborFailure {
    fn from(error: CborError) -> Self {
        CborFailure::Cbor(error)
    }
}

impl From<RangeError> for CborFailure {
    fn from(error: RangeError) -> Self {
        CborFailure::Range(error)
    }
}

impl fmt::Display for CborFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CborFailure {}

fn resolve_limit(name: &str, value: u64, maximum: u64) -> Result<u64, RangeError> {
    if value > maximum {
        return Err(RangeError::new(format!(
            "{name} must be an integer between 0 and {maximum}"
        )));
    }
    Ok(value)
}

/// Port of upstream `resolveOptions`.
pub fn resolve_options(options: CborOptions) -> Result<ResolvedCborOptions, RangeError> {
    Ok(ResolvedCborOptions {
        max_byte_length: resolve_limit(
            "maxByteLength",
            options
                .max_byte_length
                .unwrap_or(DEFAULT_MAX_CBOR_BYTE_LENGTH),
            u64::from(MAX_UINT32),
        )?,
        max_container_length: resolve_limit(
            "maxContainerLength",
            options
                .max_container_length
                .unwrap_or(DEFAULT_MAX_CBOR_CONTAINER_LENGTH),
            u64::from(MAX_UINT32),
        )?,
        max_depth: resolve_limit(
            "maxDepth",
            options.max_depth.unwrap_or(DEFAULT_MAX_CBOR_DEPTH),
            MAX_CONFIGURED_DEPTH,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults() {
        let resolved = resolve_options(CborOptions::new()).unwrap();
        assert_eq!(resolved.max_byte_length, 16 * 1024 * 1024);
        assert_eq!(resolved.max_container_length, 1_000_000);
        assert_eq!(resolved.max_depth, 64);
    }

    #[test]
    fn rejects_representable_overflows_with_upstream_text() {
        // D4: non-integer / negative inputs are type-level impossible; the
        // representable range overflows keep the exact upstream messages.
        let error =
            resolve_options(CborOptions::new().with_max_byte_length(4_294_967_296)).unwrap_err();
        assert_eq!(
            error.message(),
            "maxByteLength must be an integer between 0 and 4294967295"
        );
        let error = resolve_options(CborOptions::new().with_max_depth(513)).unwrap_err();
        assert_eq!(
            error.message(),
            "maxDepth must be an integer between 0 and 512"
        );
        let error = resolve_options(CborOptions::new().with_max_container_length(4_294_967_296))
            .unwrap_err();
        assert_eq!(
            error.message(),
            "maxContainerLength must be an integer between 0 and 4294967295"
        );
        assert_eq!(
            resolve_options(CborOptions::new().with_max_depth(0))
                .unwrap()
                .max_depth,
            0
        );
    }
}
