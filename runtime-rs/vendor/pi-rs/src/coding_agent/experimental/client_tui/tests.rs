//! Tests for `client_tui.rs`: pinned to the node oracle
//! (tests/fixtures/experimental_final_oracle/oracle_misc_out.json, `clientTui`
//! section).

use super::*;

#[test]
fn prompt_parsing_matches_the_oracle() {
    // Oracle: clientTui.parse.
    assert_eq!(
        parse_prompt_input("hello"),
        PromptInput::Prompt("hello".to_string())
    );
    assert_eq!(
        parse_prompt_input("  hello world  "),
        PromptInput::Prompt("hello world".to_string())
    );
    assert_eq!(
        parse_prompt_input("/model"),
        PromptInput::Slash {
            name: "model".to_string(),
            args: String::new(),
        }
    );
    assert_eq!(
        parse_prompt_input("/reload now"),
        PromptInput::Slash {
            name: "reload".to_string(),
            args: "now".to_string(),
        }
    );
    // Interior whitespace in args is preserved (upstream slice + trim).
    assert_eq!(
        parse_prompt_input("/a  b   c "),
        PromptInput::Slash {
            name: "a".to_string(),
            args: "b   c".to_string(),
        }
    );
    assert_eq!(parse_prompt_input("   "), PromptInput::Empty);
}

#[test]
fn footer_matches_the_oracle() {
    // Oracle: clientTui.footer.
    assert_eq!(footer_text(None), "/model · /thinking · /compact · /reload");
    assert_eq!(
        footer_text(Some(&FooterSnapshot {
            model_provider: "test".to_string(),
            model_id: "one".to_string(),
            thinking_level: "off".to_string(),
            message_count: 7,
        })),
        "test/one · thinking:off · 7 messages · /model · /thinking · /compact · /reload"
    );
}

#[test]
fn reports_match_the_oracle() {
    // Oracle: clientTui.reports.
    assert_eq!(
        report_operation(&OperationReport {
            accepted: true,
            error: None,
        }),
        ""
    );
    assert_eq!(
        report_operation(&OperationReport {
            accepted: true,
            error: Some("mid-turn".to_string()),
        }),
        "Operation failed: mid-turn"
    );
    assert_eq!(
        report_operation(&OperationReport {
            accepted: false,
            error: Some("busy".to_string()),
        }),
        "Operation rejected: busy"
    );
    assert_eq!(
        report_queue(&QueueReport {
            accepted: true,
            entry_id: Some("entry-9".to_string()),
            error: None,
        }),
        "Queued entry-9."
    );
    assert_eq!(
        report_queue(&QueueReport {
            accepted: false,
            entry_id: None,
            error: Some("queue full".to_string()),
        }),
        "Message rejected: queue full"
    );
}

#[test]
fn session_selection_matches_the_oracle() {
    // Oracle: clientTui.selection.
    let servers = vec![
        vec![
            TuiSessionSummary {
                server_id: "server-a".to_string(),
                session_id: "one".to_string(),
                created_at: 1,
            },
            TuiSessionSummary {
                server_id: "server-a".to_string(),
                session_id: "two".to_string(),
                created_at: 5,
            },
        ],
        vec![TuiSessionSummary {
            server_id: "server-b".to_string(),
            session_id: "three".to_string(),
            created_at: 5,
        }],
        vec![],
    ];
    let server_ids = [
        "server-a".to_string(),
        "server-b".to_string(),
        "server-c".to_string(),
    ];

    // continue_/resume: newest createdAt wins; the tie between "two" (5,
    // server-a) and "three" (5, server-b) resolves on serverId ascending.
    assert_eq!(
        prepare_client_session_selection(None, true, false, false, &server_ids, &servers).unwrap(),
        SessionSelection::Existing(TuiSessionSummary {
            server_id: "server-a".to_string(),
            session_id: "two".to_string(),
            created_at: 5,
        })
    );
    assert_eq!(
        prepare_client_session_selection(None, false, true, false, &server_ids, &servers).unwrap(),
        SessionSelection::Existing(TuiSessionSummary {
            server_id: "server-a".to_string(),
            session_id: "two".to_string(),
            created_at: 5,
        })
    );

    // new_: single empty server -> CreateNew.
    assert_eq!(
        prepare_client_session_selection(
            None,
            false,
            false,
            false,
            &server_ids[2..],
            &[servers[2].clone()]
        )
        .unwrap(),
        SessionSelection::CreateNew {
            server_id: "server-c".to_string(),
        }
    );

    // newRequiresSingle: two-plus servers with no selection -> error.
    assert_eq!(
        prepare_client_session_selection(None, false, false, false, &server_ids, &servers)
            .unwrap_err(),
        "Starting a Session requires exactly one server"
    );

    // explicit: existing session on server-b.
    assert_eq!(
        prepare_client_session_selection(Some("three"), false, false, false, &server_ids, &servers)
            .unwrap(),
        SessionSelection::Existing(TuiSessionSummary {
            server_id: "server-b".to_string(),
            session_id: "three".to_string(),
            created_at: 5,
        })
    );

    // explicitAmbiguous.
    let duplicated = vec![
        servers[0].clone(),
        vec![TuiSessionSummary {
            server_id: "server-d".to_string(),
            session_id: "dup".to_string(),
            created_at: 0,
        }],
        vec![TuiSessionSummary {
            server_id: "server-e".to_string(),
            session_id: "dup".to_string(),
            created_at: 0,
        }],
    ];
    assert_eq!(
        prepare_client_session_selection(
            Some("dup"),
            false,
            false,
            false,
            &server_ids,
            &duplicated
        )
        .unwrap_err(),
        "Session dup is available from more than one server"
    );

    // explicitRemoteMissing.
    assert_eq!(
        prepare_client_session_selection(Some("nope"), false, false, true, &server_ids, &servers)
            .unwrap_err(),
        "Remote server does not contain Session nope"
    );

    // A missing session on a single unix server creates it with that id.
    assert_eq!(
        prepare_client_session_selection(
            Some("fresh"),
            false,
            false,
            false,
            &server_ids[..1],
            &[servers[0].clone()],
        )
        .unwrap(),
        SessionSelection::CreateWithId {
            server_id: "server-a".to_string(),
            session_id: "fresh".to_string(),
        }
    );
}

#[test]
fn connection_state_transitions_match_upstream() {
    // Upstream #handleConnectionState / #handleAttachmentState.
    let mut state = TuiStateFacade {
        selected_server_id: Some("server-1".to_string()),
        session_id: Some("session-1".to_string()),
        ..Default::default()
    };

    // Disconnected: close the lane and announce the retry.
    assert_eq!(
        state.handle_connection_state("server-1", &ConnectionState::Disconnected),
        RecoveryAction::CloseLane
    );
    assert_eq!(state.status, "Radius disconnected; retrying…");
    assert!(state.busy);

    // Connecting mid-retry: the reconnecting announcement.
    assert_eq!(
        state.handle_connection_state("server-1", &ConnectionState::Connecting),
        RecoveryAction::CloseLane
    );
    assert_eq!(state.status, "Reconnecting to Radius…");

    // Connected with a closed lane: reattach status, lane reopen pending.
    assert_eq!(
        state.handle_connection_state("server-1", &ConnectionState::Connected),
        RecoveryAction::ReopenLane
    );
    assert_eq!(state.status, "Reattaching Session…");

    // Attached for our session: clear busy and status once the lane is open.
    state.lane_open = true;
    assert_eq!(
        state.handle_attachment_state(
            "server-1",
            &AttachmentState::Attached {
                session_id: "session-1".to_string(),
            }
        ),
        RecoveryAction::None
    );
    assert_eq!(state.status, "");
    assert!(!state.busy);

    // Attaching for our session: reattach status again.
    assert_eq!(
        state.handle_attachment_state(
            "server-1",
            &AttachmentState::Attaching {
                session_id: "session-1".to_string(),
            }
        ),
        RecoveryAction::None
    );
    assert_eq!(state.status, "Reattaching Session…");

    // Another server's states are ignored.
    assert_eq!(
        state.handle_connection_state("server-2", &ConnectionState::Disconnected),
        RecoveryAction::None
    );
}

#[test]
fn recovery_errors_and_interrupt_match_upstream_strings() {
    let mut state = TuiStateFacade::default();
    state.recovery_failed("socket hung up");
    assert_eq!(state.status, "Reconnect error: socket hung up");
    assert_eq!(state.interrupt(Some("run-1")).as_deref(), Some("run-1"));
    assert_eq!(state.status, "Aborting run-1…");
    assert_eq!(state.interrupt(None), None);
    state.show_error("attach failed");
    assert_eq!(state.status, "Error: attach failed");
}

#[test]
fn steering_decision_matches_upstream() {
    assert!(submit_is_steering(true));
    assert!(!submit_is_steering(false));
    assert!(is_canonical_server_id(
        "00000000-0000-4000-8000-000000000001"
    ));
    assert!(!is_canonical_server_id("invalid"));
}
