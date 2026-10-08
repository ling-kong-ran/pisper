//! Remote service error taxonomy. Port of
//! `packages/chord/src/services/errors.ts` (upstream sha256
//! `487e73b60a1c69dd132cf6f4c4ec625e346b5cecc1cb890da25a5199a43798a8`).

use std::fmt;

/// `REMOTE_SERVICE_ERROR_CODES` (`errors.ts:1-10`).
pub const REMOTE_SERVICE_ERROR_CODES: [&str; 8] = [
    "service_not_allowed",
    "service_not_found",
    "service_mode_mismatch",
    "service_member_not_found",
    "service_member_mismatch",
    "service_instance_not_found",
    "service_stale_instance",
    "service_invalid_value",
];

/// `RemoteServiceErrorCode` (`errors.ts:12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteServiceErrorCode {
    ServiceNotAllowed,
    ServiceNotFound,
    ServiceModeMismatch,
    ServiceMemberNotFound,
    ServiceMemberMismatch,
    ServiceInstanceNotFound,
    ServiceStaleInstance,
    ServiceInvalidValue,
}

impl RemoteServiceErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteServiceErrorCode::ServiceNotAllowed => "service_not_allowed",
            RemoteServiceErrorCode::ServiceNotFound => "service_not_found",
            RemoteServiceErrorCode::ServiceModeMismatch => "service_mode_mismatch",
            RemoteServiceErrorCode::ServiceMemberNotFound => "service_member_not_found",
            RemoteServiceErrorCode::ServiceMemberMismatch => "service_member_mismatch",
            RemoteServiceErrorCode::ServiceInstanceNotFound => "service_instance_not_found",
            RemoteServiceErrorCode::ServiceStaleInstance => "service_stale_instance",
            RemoteServiceErrorCode::ServiceInvalidValue => "service_invalid_value",
        }
    }
}

/// `isRemoteServiceErrorCode(value)` (`errors.ts:14-16`).
pub fn is_remote_service_error_code(value: &str) -> bool {
    REMOTE_SERVICE_ERROR_CODES.contains(&value)
}

/// Error surface for the chord services port. `Remote` is the upstream
/// `RemoteServiceError` (`errors.ts:18-26`) — the port keeps its `code` and
/// message. `Type` stands in for the bare `TypeError`s the upstream modules
/// throw for shape violations. `Aggregate` is the upstream `AggregateError`
/// shape used when several listener/subscription failures are collected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordError {
    Remote {
        code: RemoteServiceErrorCode,
        message: String,
    },
    /// Upstream `TypeError`/`Error` with a plain message.
    Type(String),
    /// Upstream `AggregateError(errors, message)`.
    Aggregate {
        message: String,
        errors: Vec<ChordError>,
    },
}

impl ChordError {
    /// `new RemoteServiceError(code, message)` (`errors.ts:21-23`).
    pub fn remote(code: RemoteServiceErrorCode, message: impl Into<String>) -> ChordError {
        ChordError::Remote {
            code,
            message: message.into(),
        }
    }

    /// The error message, matching the upstream `error.message` text.
    pub fn message(&self) -> &str {
        match self {
            ChordError::Remote { message, .. } | ChordError::Type(message) => message,
            ChordError::Aggregate { message, .. } => message,
        }
    }

    /// The remote error code, when this is one.
    pub fn code(&self) -> Option<RemoteServiceErrorCode> {
        match self {
            ChordError::Remote { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ChordError {}

impl From<crate::chord::delta::DeltaError> for ChordError {
    /// Delta validation errors surface as plain type errors on the services
    /// surface, matching the upstream `TypeError`s.
    fn from(error: crate::chord::delta::DeltaError) -> ChordError {
        ChordError::Type(error.message().to_owned())
    }
}
