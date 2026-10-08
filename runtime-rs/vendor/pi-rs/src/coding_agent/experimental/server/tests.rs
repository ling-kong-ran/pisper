//! Tests for `server.rs`: pinned to the node oracle captured from verbatim
//! upstream bodies (tests/fixtures/experimental_final_oracle/oracle_misc_out.json,
//! `server`, `serverLifetime`, `serverProfile` sections) plus the upstream
//! `experimental-server-profile.test.ts` scenarios.

use super::*;

#[test]
fn server_directory_resolution_defaults_to_the_home_layout() {
    // Upstream resolveServerDirectory default: ~/.pi/server, resolved.
    let resolved = resolve_server_directory(Some("relative/dir"), Some("/env/server"));
    assert!(Path::new(&resolved).is_absolute());
    assert!(resolved.replace('\\', "/").ends_with("relative/dir"));
    let from_env = resolve_server_directory(None, Some("/env/server"));
    assert!(from_env.replace('\\', "/").ends_with("env/server"));
    let default = resolve_server_directory(None, None);
    assert!(default.replace('\\', "/").contains(".pi/server"));
}

#[test]
fn session_directory_defaults_under_the_agent_dir() {
    let resolved = resolve_session_directory(Some("rel/sessions"));
    assert!(Path::new(&resolved).is_absolute());
    assert!(resolved.replace('\\', "/").ends_with("rel/sessions"));
    let default = resolve_session_directory(None);
    assert!(default.replace('\\', "/").contains("experimental/sessions"));
}

#[test]
#[cfg(not(unix))]
fn private_directory_guard_requires_a_posix_user_id() {
    // Upstream `process.getuid` is not a function on Windows.
    let error = ensure_private_server_directory("C:/nonexistent-dir").unwrap_err();
    assert_eq!(error, "Unix socket directory requires a POSIX user ID");
}

#[test]
fn server_profile_serializes_launchers_and_preserves_the_identity() {
    // Upstream experimental-server-profile.test.ts scenario 1-3 + oracle.
    let directory = std::env::temp_dir().join(format!("pi_rust_profile_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let dir = directory.to_string_lossy().to_string();

    let first = acquire_server_profile(&dir, None).unwrap();
    let first_id = first.server_id.clone();
    // Oracle: uuidShape — the created identity is a canonical UUIDv4.
    assert!(is_connection_id(&first_id));
    // Oracle: sameAsReread — after the holder releases, re-acquiring reads
    // the stored default identity.
    first.release().unwrap();
    let second = acquire_server_profile(&dir, None).unwrap();
    assert_eq!(second.server_id, first_id);
    second.release().unwrap();
    let stored = std::fs::read_to_string(directory.join("default-server-id")).unwrap();
    assert_eq!(stored.trim(), first_id);
    // Oracle: explicit / explicitLock.
    let explicit =
        acquire_server_profile(&dir, Some("00000000-0000-4000-8000-000000000001")).unwrap();
    assert_eq!(explicit.server_id, "00000000-0000-4000-8000-000000000001");
    assert!(directory
        .join("launcher-00000000-0000-4000-8000-000000000001")
        .exists());
    explicit.release().unwrap();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn different_server_ids_do_not_serialize() {
    // Upstream experimental-server-profile.test.ts scenario 2.
    let directory =
        std::env::temp_dir().join(format!("pi_rust_profile_multi_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let dir = directory.to_string_lossy().to_string();
    let first = acquire_server_profile(&dir, Some("00000000-0000-4000-8000-000000000001")).unwrap();
    let second =
        acquire_server_profile(&dir, Some("00000000-0000-4000-8000-000000000002")).unwrap();
    assert_eq!(first.server_id, "00000000-0000-4000-8000-000000000001");
    assert_eq!(second.server_id, "00000000-0000-4000-8000-000000000002");
    let mut entries: Vec<String> = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            "launcher-00000000-0000-4000-8000-000000000001",
            "launcher-00000000-0000-4000-8000-000000000002"
        ]
    );
    first.release().unwrap();
    second.release().unwrap();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn corrupt_and_invalid_identities_match_the_oracle_errors() {
    // Oracle: serverProfile.corrupt / invalidId (upstream
    // experimental-server-profile.test.ts scenarios 4-5).
    let directory =
        std::env::temp_dir().join(format!("pi_rust_profile_corrupt_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("default-server-id"), "invalid\n").unwrap();
    let error = acquire_server_profile(&directory.to_string_lossy(), None).unwrap_err();
    assert_eq!(
        error,
        format!(
            "Invalid default experimental server identity in {}",
            directory.join("default-server-id").display()
        )
    );

    let directory2 =
        std::env::temp_dir().join(format!("pi_rust_profile_invalid_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory2);
    std::fs::create_dir_all(&directory2).unwrap();
    let error = acquire_server_profile(&directory2.to_string_lossy(), Some("invalid")).unwrap_err();
    assert_eq!(error, "Invalid experimental server ID: invalid");
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&directory2);
}

#[test]
fn lock_files_serialize_the_same_identity() {
    // Oracle: serverProfile.first.lockName — holding the launcher lock blocks
    // a second acquirer (the oracle asserts `secondAcquired === false` while
    // the first is held; here the same exclusivity surfaces as an error).
    let directory = std::env::temp_dir().join(format!("pi_rust_lock_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("launcher-00000000-0000-4000-8000-000000000001");
    let guard = FileLockGuard::acquire(&path, 1, 1, 1, 1).unwrap();
    let error = FileLockGuard::acquire(&path, 1, 1, 5, 1).unwrap_err();
    assert!(error.contains("Could not acquire the experimental server lock"));
    guard.release().unwrap();
    // After release the lock is acquirable again.
    let guard = FileLockGuard::acquire(&path, 1, 1, 5, 1).unwrap();
    guard.release().unwrap();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn model_options_match_the_oracle_matrix() {
    // Oracle: server.modelOptions (null = Ok(None)).
    type ModelCase<'a> = (&'a str, Result<Option<(Option<&'a str>, &'a str)>, &'a str>);
    let cases: [ModelCase; 10] = [
        ("__absent__", Ok(None)),
        (r#"{"model":"m1"}"#, Ok(Some((None, "m1")))),
        (
            r#"{"provider":"p","model":"m1"}"#,
            Ok(Some((Some("p"), "m1"))),
        ),
        (
            "not json",
            Err("Internal server received invalid model options"),
        ),
        ("[1]", Err("Internal server received invalid model options")),
        (
            r#"{"model":""}"#,
            Err("Internal server received invalid model options"),
        ),
        (
            r#"{"model":"m","extra":1}"#,
            Err("Internal server received invalid model options"),
        ),
        (
            r#"{"model":5}"#,
            Err("Internal server received invalid model options"),
        ),
        (
            r#"{"provider":"","model":"m"}"#,
            Err("Internal server received invalid model options"),
        ),
        (
            r#"{"provider":"p"}"#,
            Err("Internal server received invalid model options"),
        ),
    ];
    for (input, expected) in cases {
        let actual = parse_server_model_options(if input == "__absent__" {
            None
        } else {
            Some(input)
        });
        match expected {
            Ok(None) => assert_eq!(actual.unwrap(), None, "case {input}"),
            Ok(Some((provider, model))) => assert_eq!(
                actual.unwrap(),
                Some(ServerModelOptions {
                    provider: provider.map(str::to_string),
                    model: model.to_string(),
                }),
                "case {input}"
            ),
            Err(message) => assert_eq!(actual.unwrap_err(), message, "case {input}"),
        }
    }
    // JSON null provider is a non-string: invalid (upstream typeof check).
    let error = parse_server_model_options(Some(r#"{"provider":null,"model":"m"}"#)).unwrap_err();
    assert_eq!(error, "Internal server received invalid model options");
}

#[test]
fn same_strings_matches_the_oracle() {
    // Oracle: server.sameStrings.
    assert!(same_strings(
        &["a".to_string(), "b".to_string()],
        &["a".to_string(), "b".to_string()]
    ));
    assert!(!same_strings(
        &["a".to_string()],
        &["a".to_string(), "b".to_string()]
    ));
    assert!(same_strings(&[], &[]));
    assert!(!same_strings(&["a".to_string()], &["b".to_string()]));
}

#[test]
fn server_process_args_match_the_oracle_errors() {
    // Oracle: server.args.
    let error = validate_server_process_args(
        &[
            "dir".to_string(),
            "x".to_string(),
            "s".to_string(),
            "m".to_string(),
            "extra".to_string(),
        ],
        |_| true,
    )
    .unwrap_err();
    assert_eq!(error, "Internal server received unexpected arguments");

    let error = validate_server_process_args(
        &[
            "dir".to_string(),
            "00000000-0000-4000-8000-000000000001".to_string(),
            "s".to_string(),
        ],
        |_| false,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "Internal server requires an absolute server directory"
    );

    let slash_absolute = |value: &str| value.starts_with('/');
    let error = validate_server_process_args(
        &["/d".to_string(), "bad-id".to_string(), "/s".to_string()],
        slash_absolute,
    )
    .unwrap_err();
    assert_eq!(error, "Internal server requires a canonical server ID");

    // Valid canonical invocation (oracle "no error" case uses absolute paths).
    let args = validate_server_process_args(
        &[
            "/d".to_string(),
            "00000000-0000-4000-8000-000000000001".to_string(),
            "/s".to_string(),
        ],
        slash_absolute,
    )
    .unwrap();
    assert_eq!(args.server_id, "00000000-0000-4000-8000-000000000001");
    assert_eq!(args.serialized_model, None);
}

#[test]
fn server_lifetime_matches_the_oracle_decision_sequences() {
    // Oracle: serverLifetime — the same scenarios with the upstream grace
    // constants (10s startup, 1s idle) evaluated on an injected clock.
    let now = 1_000u64;

    // keepAlive: no startup hold, never retires.
    let mut lifetime = ServerLifetime::new(true);
    lifetime.start(now);
    assert_eq!(lifetime.startup_deadline(), None);
    assert_eq!(lifetime.retirement_deadline(), None);
    lifetime.fire_startup_expiry(now + 60_000);
    assert!(!lifetime.fire_retirement(now + 60_000));

    // keepAliveFalseStartupExpiry: retires once the startup grace lapses and
    // the idle grace follows.
    let mut lifetime = ServerLifetime::new(false);
    lifetime.start(now);
    assert_eq!(
        lifetime.startup_deadline(),
        Some(now + AUTO_SERVER_STARTUP_GRACE_MS)
    );
    assert_eq!(lifetime.retirement_deadline(), None);
    lifetime.fire_startup_expiry(now + AUTO_SERVER_STARTUP_GRACE_MS + 1);
    let deadline = lifetime.retirement_deadline().unwrap();
    assert_eq!(
        deadline,
        now + AUTO_SERVER_STARTUP_GRACE_MS + 1 + AUTO_SERVER_IDLE_GRACE_MS
    );
    assert!(!lifetime.fire_retirement(now + AUTO_SERVER_STARTUP_GRACE_MS + 1));
    assert!(lifetime.fire_retirement(deadline + 1));

    // connectionCancelsStartup: a connection during the startup hold cancels
    // it, and idle-again schedules retirement.
    let mut lifetime = ServerLifetime::new(false);
    lifetime.start(now);
    lifetime.set_connection_count(1, now);
    assert_eq!(lifetime.startup_deadline(), None);
    lifetime.set_connection_count(0, now + AUTO_SERVER_STARTUP_GRACE_MS + 60_000);
    let deadline = lifetime.retirement_deadline().unwrap();
    assert!(lifetime.fire_retirement(deadline + 1));

    // workerHoldsServer: a keepAlive server never schedules retirement
    // (oracle workerHoldsServer trace is empty).
    let mut lifetime = ServerLifetime::new(true);
    lifetime.start(now);
    lifetime.set_worker_count(1, now);
    assert_eq!(lifetime.retirement_deadline(), None);
    lifetime.set_connection_count(0, now);
    assert_eq!(lifetime.retirement_deadline(), None);
    lifetime.set_worker_count(0, now);
    assert_eq!(lifetime.retirement_deadline(), None);
    assert!(!lifetime.fire_retirement(now + 60_000));
}

#[test]
fn uuid_generator_produces_canonical_uuidv4() {
    let first = new_uuid_v4();
    let second = new_uuid_v4();
    assert!(is_connection_id(&first));
    assert!(is_connection_id(&second));
    assert_ne!(first, second);
}

#[test]
fn foreground_assembly_serializes_activation_and_releases_on_failure() {
    // Upstream startForegroundServer: profile -> release -> activation ->
    // assemble -> release, even when assembly fails. On Windows the POSIX
    // guard runs first, so only the guard text is observable there.
    let directory = std::env::temp_dir().join(format!("pi_rust_fg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let dir = directory.to_string_lossy().to_string();
    let guard = ensure_private_server_directory(&dir);
    if cfg!(unix) {
        guard.unwrap();
        let result = start_foreground_server_assembly(
            &dir,
            Some("00000000-0000-4000-8000-000000000001"),
            |_directory, _server_id| Err("boom".to_string()),
        );
        assert_eq!(result.unwrap_err(), "boom");
        // The activation lock is released afterwards.
        let activation =
            acquire_server_activation(&dir, "00000000-0000-4000-8000-000000000001").unwrap();
        activation.release().unwrap();
    } else {
        assert_eq!(
            guard.unwrap_err(),
            "Unix socket directory requires a POSIX user ID"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn aggregate_cleanup_matches_the_upstream_shapes() {
    // Upstream allSettled aggregation: single failure bare, several grouped.
    assert_eq!(aggregate_cleanup(vec![], "ignored"), Ok(()));
    assert_eq!(
        aggregate_cleanup(vec![CleanupFailure("one".to_string())], "Group failed"),
        Err("one".to_string())
    );
    assert_eq!(
        aggregate_cleanup(
            vec![
                CleanupFailure("one".to_string()),
                CleanupFailure("two".to_string())
            ],
            "Group failed"
        )
        .unwrap_err(),
        "Group failed: [one, two]"
    );
}

/// In-memory [`ServerAssemblyHost`] recording the step order.
#[derive(Default)]
struct MockHost {
    steps: Vec<&'static str>,
    fail_at: Option<&'static str>,
    replaced: bool,
    lease: Option<u64>,
}

impl MockHost {
    fn step(&mut self, name: &'static str) -> Result<(), String> {
        self.steps.push(name);
        if self.fail_at == Some(name) {
            Err(format!("{name} exploded"))
        } else {
            Ok(())
        }
    }
}

impl ServerAssemblyHost for MockHost {
    fn ensure_coordinator(
        &mut self,
        _socket_path: &str,
        _control_path: &str,
    ) -> Result<u64, String> {
        self.steps.push("ensure_coordinator");
        self.lease = Some(7);
        Ok(7)
    }
    fn start_backend(&mut self, _server_path: &str) -> Result<(), String> {
        self.step("start_backend")
    }
    fn connect_coordinator(&mut self) -> Result<Vec<String>, String> {
        self.steps.push("connect_coordinator");
        Ok(vec!["peer-1".to_string()])
    }
    fn start_workers(&mut self, peer_ids: &[String]) -> Result<(), String> {
        self.steps.push("start_workers");
        assert_eq!(peer_ids, vec!["peer-1"]);
        Ok(())
    }
    fn refresh_sessions(&mut self) -> Result<(), String> {
        self.step("refresh_sessions")
    }
    fn start_relay(&mut self) -> Result<(), String> {
        self.step("start_relay")
    }
    fn close_relay(&mut self) -> Result<(), String> {
        self.steps.push("close_relay");
        Ok(())
    }
    fn close_backend(&mut self) -> Result<(), String> {
        self.steps.push("close_backend");
        Ok(())
    }
    fn shutdown_workers(&mut self) -> Result<(), String> {
        self.steps.push("shutdown_workers");
        Ok(())
    }
    fn close_coordinator(&mut self) -> Result<(), String> {
        self.steps.push("close_coordinator");
        Ok(())
    }
    fn coordinator_was_replaced(&self) -> bool {
        self.replaced
    }
    fn release_profile(&mut self) -> Result<(), String> {
        self.steps.push("release_profile");
        Ok(())
    }
    fn detach_workers(&mut self) {
        self.steps.push("detach_workers");
    }
}

#[test]
fn start_server_assembly_follows_the_upstream_order() {
    let mut host = MockHost::default();
    let handle = start_server_assembly(
        &mut host,
        "/dir/control.sock",
        "/dir/control-<id>.sock",
        "/dir/server-<id>-<nonce>.sock",
        "/sessions",
    )
    .unwrap();
    assert_eq!(
        host.steps,
        vec![
            "ensure_coordinator",
            "start_backend",
            "connect_coordinator",
            "start_workers",
            "refresh_sessions",
            "start_relay",
        ]
    );
    assert_eq!(handle.socket_path, "/dir/control.sock");
    assert_eq!(handle.server_path, "/dir/server-<id>-<nonce>.sock");
    assert_eq!(handle.session_dir, "/sessions");
    assert_eq!(host.lease, Some(7));
}

#[test]
fn start_server_assembly_cleanup_matches_the_upstream_order() {
    let mut host = MockHost {
        fail_at: Some("start_relay"),
        ..Default::default()
    };
    let error = start_server_assembly(&mut host, "/c", "/ctl", "/srv", "/s").unwrap_err();
    assert_eq!(error, "start_relay exploded");
    assert_eq!(
        host.steps,
        vec![
            "ensure_coordinator",
            "start_backend",
            "connect_coordinator",
            "start_workers",
            "refresh_sessions",
            "start_relay",
            "close_relay",
            "close_backend",
            "shutdown_workers",
            "close_coordinator",
            "release_profile",
        ]
    );

    // A replaced coordinator detaches the workers and skips their shutdown.
    let mut host = MockHost {
        fail_at: Some("refresh_sessions"),
        replaced: true,
        ..Default::default()
    };
    let error = start_server_assembly(&mut host, "/c", "/ctl", "/srv", "/s").unwrap_err();
    assert_eq!(error, "refresh_sessions exploded");
    assert!(host.steps.contains(&"detach_workers"));
    assert!(!host.steps.contains(&"shutdown_workers"));
}
