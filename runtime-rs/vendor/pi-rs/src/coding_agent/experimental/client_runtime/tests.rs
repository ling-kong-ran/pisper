//! Tests for `client_runtime.rs`: pinned to the node oracle
//! (tests/fixtures/experimental_final_oracle/oracle_misc_out.json, `clientRuntime`
//! section) plus upstream behavior notes.

use super::*;

fn command(base: ClientCommand) -> ClientCommand {
    base
}

#[test]
fn open_option_validation_matches_the_oracle_matrix() {
    // Oracle: clientRuntime.validation.
    let auth = CommandAuthInput::Token("t".to_string());
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            auth: Some(auth.clone()),
            ..Default::default()
        }))
        .unwrap_err(),
        "Authentication is only supported for experimental Radius connections"
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            auth: Some(auth.clone()),
            connect: Some(ConnectTarget::Radius {
                server_id: "00000000-0000-4000-8000-000000000001".to_string(),
            }),
            ..Default::default()
        })),
        Ok(())
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            provider: Some("p".to_string()),
            ..Default::default()
        }))
        .unwrap_err(),
        "Server model provider requires a model"
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            provider: Some("p".to_string()),
            model: Some("m".to_string()),
            ..Default::default()
        })),
        Ok(())
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            connect: Some(ConnectTarget::Unix {
                path: "/run/x.sock".to_string()
            }),
            model: Some("m".to_string()),
            ..Default::default()
        }))
        .unwrap_err(),
        "Model selection is only valid when automatically activating a new server"
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            connect: Some(ConnectTarget::Radius {
                server_id: "00000000-0000-4000-8000-000000000001".to_string(),
            }),
            plugin_packages: Some(vec!["./x".to_string()]),
            ..Default::default()
        }))
        .unwrap_err(),
        "Plugin package paths can only be configured on a local Unix server"
    );
    assert_eq!(
        validate_open_options(&command(ClientCommand {
            connect: Some(ConnectTarget::Unix {
                path: "/run/x.sock".to_string()
            }),
            plugin_packages: Some(vec!["./x".to_string()]),
            ..Default::default()
        })),
        Ok(())
    );
}

#[test]
fn explicit_path_routes_match_the_oracle() {
    // Oracle: clientRuntime.routes.
    let route = route_from_explicit_path("/run/00000000-0000-4000-8000-000000000001.sock").unwrap();
    assert_eq!(
        route,
        UnixServerRoute {
            server_id: "00000000-0000-4000-8000-000000000001".to_string(),
            path: "/run/00000000-0000-4000-8000-000000000001.sock".to_string(),
        }
    );
    assert_eq!(
        route_from_explicit_path("/run/not-a-uuid.sock").unwrap_err(),
        "--connect path must end with <uuidv4-server-id>.sock"
    );
    assert_eq!(
        route_from_explicit_path("/run/no-sock-suffix").unwrap_err(),
        "--connect path must end with <uuidv4-server-id>.sock"
    );
}

#[test]
fn dispose_error_aggregation_matches_the_oracle() {
    // Oracle: clientRuntime.disposeErrors.
    assert_eq!(aggregate_dispose_errors(vec![]), Ok(()));
    assert_eq!(
        aggregate_dispose_errors(vec![DisposeFailure("one".to_string())]),
        Err("one".to_string())
    );
    assert_eq!(
        aggregate_dispose_errors(vec![
            DisposeFailure("one".to_string()),
            DisposeFailure("two".to_string())
        ])
        .unwrap_err(),
        "Failed to dispose experimental client runtime: [one, two]"
    );
    assert_eq!(
        dispose_errors_to_json(&[DisposeFailure("one".to_string())]),
        serde_json::json!(["one"])
    );
    assert_eq!(
        startup_cleanup_error("connect failed", "dispose failed"),
        "Experimental client startup and cleanup failed: [connect failed, dispose failed]"
    );
}

#[test]
fn connect_failure_classification_decides_reactivation() {
    // Upstream connect(): DisconnectedError and version ServerError trigger
    // automatic reactivation on unix routes; transient errnos report as
    // "no server"; everything else propagates.
    assert!(ConnectFailure::Disconnected.allows_reactivation());
    assert!(ConnectFailure::Version.allows_reactivation());
    assert!(!ConnectFailure::Fatal("no".to_string()).allows_reactivation());
    assert!(ConnectFailure::TransportErrno("ENOENT".to_string()).is_transient_transport_failure());
    assert!(
        ConnectFailure::TransportErrno("ECONNREFUSED".to_string()).is_transient_transport_failure()
    );
    assert!(
        ConnectFailure::TransportErrno("ECONNRESET".to_string()).is_transient_transport_failure()
    );
    assert!(ConnectFailure::TransportErrno("EPIPE".to_string()).is_transient_transport_failure());
    assert!(
        ConnectFailure::TransportErrno("ETIMEDOUT".to_string()).is_transient_transport_failure()
    );
    assert!(!ConnectFailure::TransportErrno("EACCES".to_string()).is_transient_transport_failure());
}

#[test]
fn builtin_management_remove_waits_only_for_the_current_attachment() {
    // Upstream activateBuiltinClientServices' remove wrapper.
    assert!(builtin_management_remove_waits_for_detach(
        Some("session-1"),
        "session-1"
    ));
    assert!(!builtin_management_remove_waits_for_detach(
        Some("session-2"),
        "session-1"
    ));
    assert!(!builtin_management_remove_waits_for_detach(
        None,
        "session-1"
    ));
}

#[derive(Default)]
struct MockSeam {
    discovered: Vec<UnixServerRoute>,
    activated: Vec<String>,
    connect_results: Vec<Result<String, ConnectFailure>>,
}

impl ClientRuntimeSeam for MockSeam {
    fn discover_unix_servers(&mut self, _directory: &str) -> Result<Vec<UnixServerRoute>, String> {
        Ok(self.discovered.clone())
    }
    fn activate_server(
        &mut self,
        _directory: &str,
        requested_server_id: Option<&str>,
        _session_dir: &str,
        _provider: Option<&str>,
        _model: Option<&str>,
    ) -> Result<(UnixServerRoute, String), String> {
        let server_id = requested_server_id
            .unwrap_or("00000000-0000-4000-8000-000000000009")
            .to_string();
        self.activated.push(server_id.clone());
        Ok((
            UnixServerRoute {
                server_id,
                path: "/activated.sock".to_string(),
            },
            "activated-client".to_string(),
        ))
    }
    fn connect(&mut self, _route: &ClientRuntimeRoute) -> Result<String, ConnectFailure> {
        self.connect_results.remove(0)
    }
    fn server_directory_env(&self) -> Option<String> {
        None
    }
    fn server_id_env(&self) -> Option<String> {
        None
    }
    fn session_directory(&self) -> String {
        "/sessions".to_string()
    }
}

#[test]
fn route_planning_follows_the_upstream_decisions() {
    // Discovery finds servers: no activation, no model allowed.
    let mut seam = MockSeam {
        discovered: vec![UnixServerRoute {
            server_id: "00000000-0000-4000-8000-000000000001".to_string(),
            path: "/run/00000000-0000-4000-8000-000000000001.sock".to_string(),
        }],
        ..Default::default()
    };
    let command = ClientCommand::default();
    let (routes, activated) = plan_client_routes(&command, &mut seam, Some("/dir")).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].transport(), "unix");
    assert_eq!(activated, None);

    // Discovery + model selection is rejected.
    let error =
        plan_client_routes(&command.clone_with_model(), &mut seam, Some("/dir")).unwrap_err();
    assert_eq!(
        error,
        "Model selection is only valid when automatically activating a new server"
    );

    // Empty discovery triggers automatic activation.
    let mut seam = MockSeam::default();
    let (routes, activated) = plan_client_routes(&command, &mut seam, Some("/dir")).unwrap();
    assert_eq!(activated.as_deref(), Some("activated-client"));
    assert_eq!(routes.len(), 1);
    assert_eq!(
        routes[0].server_id(),
        "00000000-0000-4000-8000-000000000009"
    );

    // Radius connect targets pass through untouched.
    let mut seam = MockSeam::default();
    let (routes, activated) = plan_client_routes(
        &ClientCommand {
            connect: Some(ConnectTarget::Radius {
                server_id: "00000000-0000-4000-8000-000000000002".to_string(),
            }),
            ..Default::default()
        },
        &mut seam,
        Some("/dir"),
    )
    .unwrap();
    assert_eq!(activated, None);
    assert_eq!(routes[0].transport(), "radius");

    // Explicit unix path routes through routeFromExplicitPath.
    let mut seam = MockSeam::default();
    let (routes, _) = plan_client_routes(
        &ClientCommand {
            connect: Some(ConnectTarget::Unix {
                path: "/run/00000000-0000-4000-8000-000000000001.sock".to_string(),
            }),
            ..Default::default()
        },
        &mut seam,
        Some("/dir"),
    )
    .unwrap();
    assert_eq!(
        routes[0].server_id(),
        "00000000-0000-4000-8000-000000000001"
    );

    // Plugin packages demand exactly one local server.
    let mut seam = MockSeam {
        discovered: vec![
            UnixServerRoute {
                server_id: "00000000-0000-4000-8000-000000000001".to_string(),
                path: "/a.sock".to_string(),
            },
            UnixServerRoute {
                server_id: "00000000-0000-4000-8000-000000000002".to_string(),
                path: "/b.sock".to_string(),
            },
        ],
        ..Default::default()
    };
    let error = plan_client_routes(
        &ClientCommand {
            plugin_packages: Some(vec!["./p".to_string()]),
            ..Default::default()
        },
        &mut seam,
        Some("/dir"),
    )
    .unwrap_err();
    assert_eq!(error, "Plugin selection requires exactly one local server");
}

impl ClientCommand {
    fn clone_with_model(&self) -> ClientCommand {
        let mut next = self.clone();
        next.model = Some("m".to_string());
        next
    }
}

#[test]
fn route_client_acquisition_reactivates_only_unix_version_failures() {
    // Upstream openClientRuntime's per-route connect decision.
    let command = ClientCommand::default();
    let route = ClientRuntimeRoute::Unix(UnixServerRoute {
        server_id: "00000000-0000-4000-8000-000000000001".to_string(),
        path: "/run/x.sock".to_string(),
    });

    // An activated client short-circuits connecting.
    let mut seam = MockSeam::default();
    let client = acquire_route_client(
        &route,
        Some("already".to_string()),
        &command,
        &mut seam,
        "/dir",
    )
    .unwrap();
    assert_eq!(client, "already");

    // A version failure reactivates the server for that route.
    let mut seam = MockSeam {
        connect_results: vec![Err(ConnectFailure::Version)],
        ..Default::default()
    };
    let client = acquire_route_client(&route, None, &command, &mut seam, "/dir").unwrap();
    assert_eq!(client, "activated-client");

    // An explicit connect target never reactivates (upstream
    // `command.connect !== undefined` guard).
    let mut seam = MockSeam {
        connect_results: vec![Err(ConnectFailure::Version)],
        ..Default::default()
    };
    let error = acquire_route_client(
        &route,
        None,
        &ClientCommand {
            connect: Some(ConnectTarget::Unix {
                path: "/run/x.sock".to_string(),
            }),
            ..Default::default()
        },
        &mut seam,
        "/dir",
    );
    assert!(error.is_err());

    // Radius routes never reactivation-reactivate.
    let radius_route = ClientRuntimeRoute::Radius {
        server_id: "00000000-0000-4000-8000-000000000002".to_string(),
    };
    let mut seam = MockSeam {
        connect_results: vec![Err(ConnectFailure::Disconnected)],
        ..Default::default()
    };
    let error = acquire_route_client(&radius_route, None, &command, &mut seam, "/dir");
    assert!(error.is_err());

    // Fatal failures propagate verbatim.
    let mut seam = MockSeam {
        connect_results: vec![Err(ConnectFailure::Fatal("permission denied".to_string()))],
        ..Default::default()
    };
    let error = acquire_route_client(&route, None, &command, &mut seam, "/dir").unwrap_err();
    assert_eq!(error, "permission denied");
}
