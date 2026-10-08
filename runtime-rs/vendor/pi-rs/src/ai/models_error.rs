//! Upstream `utils/models-error.ts` (`packages/ai/src`): the [`ModelsError`]
//! type, moved out of `auth/resolve.ts` by the unified model-catalog delta so
//! the non-auth modules (`utils/model-operations.ts`, `createProvider`) can
//! throw it without importing from auth. `auth/resolve.ts` re-exports it, so
//! the `crate::ai::auth::resolve::ModelsError` path is unchanged.

/// Upstream `ModelsErrorCode` (`utils/models-error.ts`): the failure taxonomy
/// carried by [`ModelsError`]. `as_str` returns the upstream literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsErrorCode {
    ModelSource,
    ModelValidation,
    Provider,
    Stream,
    Auth,
    OAuth,
}

impl ModelsErrorCode {
    /// The upstream literal (`"model_source"`, ..., `"oauth"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelsErrorCode::ModelSource => "model_source",
            ModelsErrorCode::ModelValidation => "model_validation",
            ModelsErrorCode::Provider => "provider",
            ModelsErrorCode::Stream => "stream",
            ModelsErrorCode::Auth => "auth",
            ModelsErrorCode::OAuth => "oauth",
        }
    }
}

/// Upstream `ModelsError` (`utils/models-error.ts`): an error carrying a
/// [`ModelsErrorCode`]. Upstream `Display` is the error `message` after
/// [`with_cause_detail`](ModelsError::with_cause) folded the cause in —
/// callers surface `error.message` only, so the underlying reason lives in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsError {
    pub code: ModelsErrorCode,
    pub message: String,
}

impl ModelsError {
    pub fn new(code: ModelsErrorCode, message: impl Into<String>) -> Self {
        ModelsError {
            code,
            message: message.into(),
        }
    }

    /// Upstream `new ModelsError(code, message, { cause })`: the cause's text
    /// is appended as `": <detail>"` when non-empty and not already part of
    /// the message (upstream `withCauseDetail` — callers surface
    /// `error.message` only, so keep the underlying reason in it).
    pub fn with_cause(
        code: ModelsErrorCode,
        message: impl Into<String>,
        cause: impl std::fmt::Display,
    ) -> Self {
        let message = message.into();
        let detail = cause.to_string();
        let detail = detail.trim();
        let message = if detail.is_empty() || message.contains(detail) {
            message
        } else {
            format!("{message}: {detail}")
        };
        ModelsError { code, message }
    }
}

impl std::fmt::Display for ModelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelsError {}
