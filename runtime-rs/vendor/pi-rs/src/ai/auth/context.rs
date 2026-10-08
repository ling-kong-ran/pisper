//! Default auth context ported from upstream
//! `packages/ai/src/auth/context.ts`: env vars from the process environment
//! (undefined in browsers upstream), and file existence with leading-`~`
//! expansion (always false in browsers upstream).

use futures::future::BoxFuture;

use super::types::AuthContext;

/// Upstream `defaultProviderAuthContext()` (context.ts:23-45): env vars from
/// `process.env`, file existence via `node:fs`. Blank (whitespace-only)
/// values are treated as unset, like the upstream
/// `value.trim().length > 0` check.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultAuthContext;

impl AuthContext for DefaultAuthContext {
    fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    }

    fn file_exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Upstream expands a leading `~` against os.homedir() by string
            // concatenation (`homedir() + path.slice(1)`), preserving the
            // separator in `~/.foo`; without a home dir the path is used
            // verbatim and misses, like the upstream access() failure.
            let resolved = match path.strip_prefix('~') {
                Some(rest) => match dirs::home_dir() {
                    Some(home) => format!("{}{}", home.display(), rest),
                    None => path.to_string(),
                },
                None => path.to_string(),
            };
            std::path::Path::new(&resolved).exists()
        })
    }
}

/// Upstream `defaultProviderAuthContext` (context.ts:23).
pub fn default_provider_auth_context() -> DefaultAuthContext {
    DefaultAuthContext
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    /// Process env is process-global; serialize env-mutating tests and restore
    /// the saved values on drop (upstream `afterEach`). The variable names
    /// here are unique to this module so other modules' env tests never
    /// interfere.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnv {
        fn clearing(vars: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = vars
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            for name in vars {
                std::env::remove_var(name);
            }
            TestEnv { _lock: lock, saved }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    const TEST_VAR: &str = "PI_TEST_DEFAULT_AUTH_CONTEXT_VAR";

    #[tokio::test]
    async fn reads_process_env_and_treats_blank_values_as_unset() {
        let _env = TestEnv::clearing(&[TEST_VAR]);
        let ctx = default_provider_auth_context();

        assert_eq!(ctx.env(TEST_VAR).await, None);

        std::env::set_var(TEST_VAR, "value");
        assert_eq!(ctx.env(TEST_VAR).await, Some("value".to_string()));

        // The upstream `value.trim().length > 0` check.
        std::env::set_var(TEST_VAR, "   ");
        assert_eq!(ctx.env(TEST_VAR).await, None);
        std::env::set_var(TEST_VAR, "");
        assert_eq!(ctx.env(TEST_VAR).await, None);
    }

    #[tokio::test]
    async fn file_exists_checks_real_and_missing_paths() {
        let ctx = default_provider_auth_context();
        let file = tempfile::Builder::new().tempfile().unwrap();
        assert!(ctx.file_exists(file.path().to_str().unwrap()).await);
        assert!(!ctx.file_exists("/definitely/not/a/real/pi-rust/path").await);
    }

    #[tokio::test]
    async fn file_exists_expands_a_leading_tilde_against_the_home_dir() {
        let ctx = default_provider_auth_context();
        let Some(home) = dirs::home_dir() else {
            return;
        };
        // A real file under the home dir addressed through `~` (only when the
        // temp dir lives inside home, e.g. not on another Windows drive).
        let file = tempfile::Builder::new()
            .prefix("pi-rust-context-test")
            .tempfile()
            .unwrap();
        let path = file.path().to_string_lossy().to_string();
        if let Some(rest) = path.strip_prefix(home.to_string_lossy().as_ref()) {
            assert!(ctx.file_exists(&format!("~{rest}")).await);
        }
        // Missing files under `~` stay false, `~` alone is the home dir.
        assert!(!ctx.file_exists("~/.definitely-not-a-pi-rust-file").await);
        assert!(ctx.file_exists("~").await);
    }
}
