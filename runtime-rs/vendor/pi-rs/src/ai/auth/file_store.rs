//! File-backed credential store for `auth.json`, ported from the upstream
//! persistence semantics of `packages/ai/src/cli.ts` (`loadAuth`/`saveAuth`)
//! and the `packages/ai/test/oauth.ts` harness (`loadAuthStorage`/
//! `saveAuthStorage`), expressed through the app-owned
//! [`CredentialStore`] trait (upstream `packages/coding-agent/src/core/
//! auth-storage.ts` `AuthStorage` is the app-level reference for the
//! read-modify-write shape).
//!
//! Document format (M2d controller ruling): `{ "<providerId>": credential }`
//! — the upstream `Record<string, Credential>` — pretty-printed with two
//! spaces, byte-compatible with upstream `JSON.stringify(auth, null, 2)`
//! ([`serde_json::to_string_pretty`] uses the same 2-space indent, `": "`
//! separators, and `{}`/`[]` for empty containers).
//!
//! Semantics:
//! - A missing file is an empty document (upstream `loadAuth`'s
//!   `existsSync` guard).
//! - An unparsable file (bad JSON, wrong credential shape) is an *empty*
//!   document, not an error — upstream `loadAuth` catches the parse failure
//!   and returns `{}`; a later write then replaces the content (exactly what
//!   upstream `login` does against a corrupt file).
//! - Every write rewrites the whole document (`saveAuth` serializes the full
//!   map; `AuthStorage.modify` writes `{ ...currentData, [provider]: next }`).
//! - `delete` always rewrites the document, even when the key was absent
//!   (upstream `AuthStorage.delete` writes the filtered map unconditionally).
//! - File hygiene follows the `oauth.ts` harness: the parent directory is
//!   created recursively with mode `0o700` and the file is chmod'd `0o600`
//!   after every save (unix; other platforms skip the mode calls).
//!
//! Deviations from upstream, disclosed:
//! - Key order: the document is stored as a `BTreeMap`, so `list` reports
//!   provider ids sorted and rewrites sort the keys. Upstream `Map` iteration
//!   preserves file insertion order. This is the established port-wide
//!   sorted-JSON-key deviation (serde_json `Map` is `BTreeMap`-backed), and
//!   the T1 round-trip oracle pinned a `BTreeMap` rewrite.
//! - Reads never resolve `!command` key configs (the coding-agent
//!   `AuthStorage.read` layer; app-owned, lands with the pi-rust app).
//! - No cross-process file lock. Upstream `cli.ts` and the `oauth.ts`
//!   harness lock nothing; the coding-agent's `proper-lockfile` is app-level.
//!   Within one process, per-provider writes serialize like
//!   [`InMemoryCredentialStore`](super::credential_store::InMemoryCredentialStore).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use super::credential_store::{CredentialStore, ModifyCallback};
use super::types::{AuthError, AuthOperationOptions, Credential, CredentialInfo};

/// The `auth.json` document (upstream `AuthStorageData`).
type AuthDocument = BTreeMap<String, Credential>;

/// File-backed credential store keyed by provider id (upstream `cli.ts`
/// `loadAuth`/`saveAuth` over one `auth.json` path).
pub struct FileCredentialStore {
    path: PathBuf,
    /// Per-provider writer locks, mirroring
    /// [`InMemoryCredentialStore`](super::credential_store::InMemoryCredentialStore):
    /// one tiny allocation per provider that ever received a write.
    locks: Mutex<std::collections::HashMap<String, std::sync::Arc<Mutex<()>>>>,
}

impl FileCredentialStore {
    /// A store over one `auth.json` path. Upstream callers pass either the
    /// cwd-relative `auth.json` (`cli.ts`) or `<agent dir>/auth.json`
    /// (coding-agent); the path is caller-owned here.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            locks: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The backing `auth.json` path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Acquire this provider's writer lock, racing the operation's
    /// cancellation token (same semantics as the in-memory store).
    async fn lock_provider(
        &self,
        provider_id: &str,
        options: &AuthOperationOptions,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, AuthError> {
        options.check()?;
        let entry = {
            let mut locks = self.locks.lock().await;
            std::sync::Arc::clone(locks.entry(provider_id.to_string()).or_default())
        };
        tokio::select! {
            guard = Mutex::lock_owned(entry) => Ok(guard),
            _ = options.cancelled() => Err(AuthError::Cancelled),
        }
    }

    /// Upstream `loadAuth`: a missing or unparsable file is an empty
    /// document; IO failures other than a missing file are storage errors.
    fn load_document(&self) -> Result<AuthDocument, AuthError> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AuthDocument::default());
            }
            Err(error) => {
                return Err(AuthError::Storage(format!(
                    "could not read {}: {error}",
                    self.path.display()
                )));
            }
        };
        // Upstream `JSON.parse` inside a `catch` returning `{}`: a corrupt
        // document (bad JSON, wrong credential shape) degrades to empty.
        Ok(serde_json::from_str(&content).unwrap_or_default())
    }

    /// Upstream `saveAuth` (`writeFileSync(AUTH_FILE, JSON.stringify(auth,
    /// null, 2))`) plus the `oauth.ts` harness hygiene: parent directory
    /// (mode `0o700` on unix) and a `0o600` file chmod after the write.
    fn save_document(&self, document: &AuthDocument) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                create_dir_private(parent).map_err(|error| {
                    AuthError::Storage(format!("could not create {}: {error}", parent.display()))
                })?;
            }
        }
        let body = serde_json::to_string_pretty(document).map_err(|error| {
            AuthError::Storage(format!("could not serialize auth.json: {error}"))
        })?;
        std::fs::write(&self.path, body).map_err(|error| {
            AuthError::Storage(format!("could not write {}: {error}", self.path.display()))
        })?;
        restrict_file_mode(&self.path).map_err(|error| {
            AuthError::Storage(format!(
                "could not restrict {}: {error}",
                self.path.display()
            ))
        })?;
        Ok(())
    }
}

/// `mkdirSync(dir, { recursive: true, mode: 0o700 })` on unix.
fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// The `oauth.ts` harness `chmodSync(AUTH_PATH, 0o600)` after every save.
fn restrict_file_mode(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

impl CredentialStore for FileCredentialStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            Ok(self.load_document()?.get(provider_id).cloned())
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            Ok(self
                .load_document()?
                .into_iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id,
                    r#type: credential.auth_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            let _guard = self.lock_provider(provider_id, options).await?;
            let document = self.load_document()?;
            let current = document.get(provider_id).cloned();
            let next = {
                let input = current.clone();
                tokio::select! {
                    result = f(input) => result?,
                    _ = options.cancelled() => return Err(AuthError::Cancelled),
                }
            };
            // Upstream re-checks the signal after the callback so an abort
            // during the callback never applies its write.
            options.check()?;
            if let Some(next) = &next {
                let mut document = document;
                document.insert(provider_id.to_string(), next.clone());
                self.save_document(&document)?;
            }
            Ok(next.or(current))
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        Box::pin(async move {
            let _guard = self.lock_provider(provider_id, options).await?;
            let mut document = self.load_document()?;
            document.remove(provider_id);
            // Upstream `AuthStorage.delete` rewrites the filtered map
            // unconditionally, so deleting the last entry leaves `{}`.
            self.save_document(&document)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::{ApiKeyCredential, OAuthCredential};
    use tempfile::TempDir;

    fn no_options() -> AuthOperationOptions {
        AuthOperationOptions::default()
    }

    fn api_key(key: &str) -> Credential {
        Credential::ApiKey(ApiKeyCredential {
            key: Some(key.to_string()),
            env: None,
            extra: BTreeMap::new(),
        })
    }

    fn oauth(access: &str, refresh: &str, expires: i64) -> Credential {
        Credential::OAuth(OAuthCredential {
            access: access.to_string(),
            refresh: refresh.to_string(),
            expires,
            extra: BTreeMap::new(),
        })
    }

    fn store_in(dir: &TempDir) -> FileCredentialStore {
        FileCredentialStore::new(dir.path().join("auth.json"))
    }

    fn read_file(dir: &TempDir) -> String {
        std::fs::read_to_string(dir.path().join("auth.json")).unwrap()
    }

    #[test]
    fn missing_file_is_an_empty_document() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        assert_eq!(
            futures::executor::block_on(store.read("p1", &no_options())).unwrap(),
            None
        );
        assert_eq!(
            futures::executor::block_on(store.list(&no_options())).unwrap(),
            Vec::new()
        );
        assert!(!dir.path().join("auth.json").exists());
    }

    #[test]
    fn a_corrupt_file_degrades_to_an_empty_document() {
        // Upstream `loadAuth` catches the `JSON.parse` failure and returns
        // `{}`; reads and lists see an empty store instead of erroring.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), "{not json").unwrap();
        let store = store_in(&dir);
        assert_eq!(
            futures::executor::block_on(store.read("p1", &no_options())).unwrap(),
            None
        );
        assert_eq!(
            futures::executor::block_on(store.list(&no_options())).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn a_shape_invalid_entry_degrades_to_an_empty_document() {
        // The port validates entries against the Credential schema at parse
        // time (upstream cli.ts casts unchecked): an oauth entry missing
        // `expires` is corrupt like a JSON syntax error.
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"p1":{"type":"oauth","refresh":"r","access":"a"}}"#,
        )
        .unwrap();
        let store = store_in(&dir);
        assert_eq!(
            futures::executor::block_on(store.read("p1", &no_options())).unwrap(),
            None
        );
    }

    #[test]
    fn modify_writes_the_upstream_pretty_document() {
        // `JSON.stringify(auth, null, 2)`: 2-space indent, `": "` separator.
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        futures::executor::block_on(store.modify(
            "anthropic",
            Box::new(|current| {
                assert_eq!(current, None);
                Box::pin(async { Ok(Some(oauth("a", "r", 10))) })
            }),
            &no_options(),
        ))
        .unwrap();
        assert_eq!(
            read_file(&dir),
            "{\n  \"anthropic\": {\n    \"type\": \"oauth\",\n    \"refresh\": \"r\",\n    \
             \"access\": \"a\",\n    \"expires\": 10\n  }\n}"
        );
    }

    #[test]
    fn the_whole_document_round_trips_upstream_bytes() {
        // An upstream-written (JS insertion order) document parses, and a
        // rewrite re-serializes entry-for-entry (keys sorted by the
        // disclosed BTreeMap deviation).
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            concat!(
                "{\n",
                "  \"openai\": {\n",
                "    \"type\": \"api_key\",\n",
                "    \"key\": \"sk\"\n",
                "  },\n",
                "  \"anthropic\": {\n",
                "    \"type\": \"oauth\",\n",
                "    \"refresh\": \"r\",\n",
                "    \"access\": \"a\",\n",
                "    \"expires\": 1735689600000,\n",
                "    \"accountId\": \"acc\"\n",
                "  }\n",
                "}"
            ),
        )
        .unwrap();
        let store = FileCredentialStore::new(&path);
        assert_eq!(
            futures::executor::block_on(store.read("anthropic", &no_options())).unwrap(),
            Some(Credential::OAuth(OAuthCredential {
                refresh: "r".to_string(),
                access: "a".to_string(),
                expires: 1_735_689_600_000,
                extra: [(
                    "accountId".to_string(),
                    serde_json::Value::String("acc".to_string())
                )]
                .into_iter()
                .collect(),
            }))
        );
        // Rewrite through modify: same entries, sorted keys, unknown OAuth
        // extension field preserved.
        futures::executor::block_on(store.modify(
            "anthropic",
            Box::new(|current| Box::pin(async move { Ok(current) })),
            &no_options(),
        ))
        .unwrap();
        assert_eq!(
            read_file(&dir),
            concat!(
                "{\n",
                "  \"anthropic\": {\n",
                "    \"type\": \"oauth\",\n",
                "    \"refresh\": \"r\",\n",
                "    \"access\": \"a\",\n",
                "    \"expires\": 1735689600000,\n",
                "    \"accountId\": \"acc\"\n",
                "  },\n",
                "  \"openai\": {\n",
                "    \"type\": \"api_key\",\n",
                "    \"key\": \"sk\"\n",
                "  }\n",
                "}"
            )
        );
    }

    #[test]
    fn api_key_unknown_fields_survive_a_file_rewrite() {
        // The T1 ruling carried here: upstream JS preserves unknown fields on
        // api_key entries through loadAuth/saveAuth (JSON.parse keeps them);
        // the port's `extra` map must too.
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"cloudflare":{"type":"api_key","key":"k","env":{"ACCOUNT_ID":"abc"},"note":"kept"}}"#,
        )
        .unwrap();
        let store = store_in(&dir);
        futures::executor::block_on(store.modify(
            "cloudflare",
            Box::new(|current| Box::pin(async move { Ok(current) })),
            &no_options(),
        ))
        .unwrap();
        assert_eq!(
            read_file(&dir),
            "{\n  \"cloudflare\": {\n    \"type\": \"api_key\",\n    \"key\": \"k\",\n    \
             \"env\": {\n      \"ACCOUNT_ID\": \"abc\"\n    },\n    \"note\": \"kept\"\n  }\n}"
        );
    }

    #[test]
    fn list_reports_sorted_entries_without_secrets() {
        // Disclosed deviation: upstream Map iteration preserves file order;
        // the BTreeMap document reports provider ids sorted.
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"zzz":{"type":"api_key","key":"k"},"aaa":{"type":"oauth","refresh":"r","access":"a","expires":1}}"#,
        )
        .unwrap();
        let store = store_in(&dir);
        assert_eq!(
            futures::executor::block_on(store.list(&no_options())).unwrap(),
            vec![
                CredentialInfo {
                    provider_id: "aaa".to_string(),
                    r#type: crate::ai::auth::types::AuthType::OAuth,
                },
                CredentialInfo {
                    provider_id: "zzz".to_string(),
                    r#type: crate::ai::auth::types::AuthType::ApiKey,
                },
            ]
        );
    }

    #[test]
    fn modify_is_a_serialized_read_modify_write_of_the_whole_document() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        futures::executor::block_on(store.modify(
            "p1",
            Box::new(|_| Box::pin(async { Ok(Some(api_key("first"))) })),
            &no_options(),
        ))
        .unwrap();
        // The second write sees the first and keeps its entry.
        futures::executor::block_on(store.modify(
            "p2",
            Box::new(|current| {
                assert_eq!(current, None);
                Box::pin(async { Ok(Some(oauth("a", "r", 1))) })
            }),
            &no_options(),
        ))
        .unwrap();
        let document: AuthDocument = serde_json::from_str(&read_file(&dir)).unwrap();
        assert_eq!(document.len(), 2);
        assert_eq!(document.get("p1"), Some(&api_key("first")));

        // Returning None leaves the document untouched on disk.
        let before = read_file(&dir);
        futures::executor::block_on(store.modify(
            "p1",
            Box::new(|_| Box::pin(async { Ok(None) })),
            &no_options(),
        ))
        .unwrap();
        assert_eq!(read_file(&dir), before);
    }

    #[test]
    fn a_modify_callback_error_propagates_and_never_writes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"p1":{"type":"api_key","key":"old"}}"#,
        )
        .unwrap();
        let store = store_in(&dir);
        let error = futures::executor::block_on(store.modify(
            "p1",
            Box::new(|_| {
                Box::pin(async { Err(AuthError::Operation("refresh failed".to_string())) })
            }),
            &no_options(),
        ))
        .unwrap_err();
        assert_eq!(error, AuthError::Operation("refresh failed".to_string()));
        // A failed callback never writes: the file keeps the raw bytes it
        // was seeded with (still the un-pretty single-line document).
        assert_eq!(read_file(&dir), r#"{"p1":{"type":"api_key","key":"old"}}"#);
    }

    #[test]
    fn a_cancelled_modify_never_writes() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let token = tokio_util::sync::CancellationToken::new();
        let options = AuthOperationOptions::new(token.clone());
        token.cancel();
        let error = futures::executor::block_on(store.modify(
            "p1",
            Box::new(|_| Box::pin(async { Ok(Some(api_key("late"))) })),
            &options,
        ))
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert!(!dir.path().join("auth.json").exists());
    }

    #[test]
    fn delete_removes_the_entry_and_rewrites_the_document() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"p1":{"type":"api_key","key":"k"},"p2":{"type":"api_key","key":"k2"}}"#,
        )
        .unwrap();
        let store = store_in(&dir);
        futures::executor::block_on(store.delete("p1", &no_options())).unwrap();
        assert_eq!(
            read_file(&dir),
            "{\n  \"p2\": {\n    \"type\": \"api_key\",\n    \"key\": \"k2\"\n  }\n}"
        );
        // Deleting the last entry leaves `{}`, like upstream's unconditional
        // filtered-map rewrite; deleting an absent entry is still fine.
        futures::executor::block_on(store.delete("p2", &no_options())).unwrap();
        assert_eq!(read_file(&dir), "{}");
        futures::executor::block_on(store.delete("absent", &no_options())).unwrap();
        assert_eq!(read_file(&dir), "{}");
    }

    #[tokio::test]
    async fn deletes_serialize_against_concurrent_modifies() {
        let dir = TempDir::new().unwrap();
        let store = std::sync::Arc::new(store_in(&dir));
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let (started, started_rx) = tokio::sync::oneshot::channel::<()>();
        let writer = store.clone();
        let write = tokio::spawn(async move {
            writer
                .modify(
                    "p1",
                    Box::new(move |_| {
                        Box::pin(async move {
                            started.send(()).ok();
                            wait.await.ok();
                            Ok(Some(api_key("written")))
                        })
                    }),
                    &no_options(),
                )
                .await
                .unwrap();
        });
        started_rx.await.unwrap();
        // The delete queues behind the in-flight modify (a yield lets it
        // reach the provider lock) and cannot lose the writer's entry.
        let deleter = store.clone();
        let delete = tokio::spawn(async move {
            deleter.delete("p1", &no_options()).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        release.send(()).ok();
        write.await.unwrap();
        delete.await.unwrap();
        // Final `{}` proves the ordering: a delete that had beaten the
        // modify would have removed nothing and the writer would have
        // re-written the entry afterwards.
        assert_eq!(read_file(&dir), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn saves_create_a_private_directory_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = FileCredentialStore::new(dir.path().join("nested").join("auth.json"));
        futures::executor::block_on(store.modify(
            "p1",
            Box::new(|_| Box::pin(async { Ok(Some(api_key("k"))) })),
            &no_options(),
        ))
        .unwrap();
        let mode = std::fs::metadata(dir.path().join("nested").join("auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
