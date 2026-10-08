//! Tests for `client.rs`: pinned to the node oracle
//! (tests/fixtures/experimental_final_oracle/oracle_misc_out.json, `client` section).

use super::*;

#[test]
fn list_branch_sorts_like_the_oracle() {
    // Oracle: client.listSort ["a/9", "b/2"].
    let result = run_client_list(&[vec![
        SessionAddress {
            server_id: "b".to_string(),
            session_id: "2".to_string(),
        },
        SessionAddress {
            server_id: "a".to_string(),
            session_id: "9".to_string(),
        },
    ]]);
    assert_eq!(
        result,
        ClientResult::List {
            sessions: vec![
                SessionAddress {
                    server_id: "a".to_string(),
                    session_id: "9".to_string(),
                },
                SessionAddress {
                    server_id: "b".to_string(),
                    session_id: "2".to_string(),
                },
            ]
        }
    );
}

#[test]
fn selection_errors_match_the_oracle() {
    // Oracle: client.errors.
    let mut created: Vec<(usize, Option<String>)> = Vec::new();
    let mut create = |server: usize, requested: Option<&str>| {
        created.push((server, requested.map(str::to_string)));
        Ok(SessionAddress {
            server_id: format!("server-{server}"),
            session_id: requested.unwrap_or("new-1").to_string(),
        })
    };
    let error = select_session_for_command(None, Some("hi"), None, &[vec![], vec![]], &mut create)
        .unwrap_err();
    assert_eq!(
        error,
        "Client prompt requires exactly one discovered server to create a Session"
    );

    let everywhere = vec![
        vec![SessionAddress {
            server_id: "a".to_string(),
            session_id: "se-1".to_string(),
        }],
        vec![SessionAddress {
            server_id: "b".to_string(),
            session_id: "se-1".to_string(),
        }],
        vec![SessionAddress {
            server_id: "c".to_string(),
            session_id: "se-1".to_string(),
        }],
    ];
    let error =
        select_session_for_command(Some("se-1"), None, None, &everywhere, &mut create).unwrap_err();
    assert_eq!(error, "Session se-1 is available from more than one server");

    // A session found on two servers is ambiguous even before the count check.
    let duplicated = vec![
        vec![SessionAddress {
            server_id: "a".to_string(),
            session_id: "se-1".to_string(),
        }],
        vec![SessionAddress {
            server_id: "b".to_string(),
            session_id: "se-1".to_string(),
        }],
    ];
    let error = select_session_for_command(Some("se-1"), Some("p"), None, &duplicated, &mut create)
        .unwrap_err();
    assert_eq!(error, "Session se-1 is available from more than one server");

    let error = select_session_for_command(
        Some("se-1"),
        Some("prompt"),
        Some("radius"),
        &[vec![]],
        &mut create,
    )
    .unwrap_err();
    assert_eq!(error, "No discovered server contains session se-1");

    let error =
        select_session_for_command(Some("se-1"), None, None, &[vec![]], &mut create).unwrap_err();
    assert_eq!(error, "No discovered server contains session se-1");

    // Missing session on one unix server with a prompt creates it there.
    let (server, session) = select_session_for_command(
        Some("se-1"),
        Some("prompt"),
        Some("unix"),
        &[vec![]],
        &mut create,
    )
    .unwrap();
    assert_eq!(server, 0);
    assert_eq!(session.session_id, "se-1");
    assert_eq!(created, vec![(0, Some("se-1".to_string()))]);
}

#[test]
fn message_text_matches_the_oracle() {
    // Oracle: client.messageText ["ab", ""].
    let joined = message_text(&[
        MessageContent::Text {
            text: "a".to_string(),
        },
        MessageContent::Other,
        MessageContent::Text {
            text: "b".to_string(),
        },
    ]);
    assert_eq!(joined, "ab");
    assert_eq!(message_text(&[]), "");
}

#[test]
fn prompt_response_errors_match_the_oracle() {
    // Oracle: client.responseErrors.
    let error = response_error(&PromptResponse {
        accepted: false,
        operation_id: None,
        error_message: Some("rejected: busy".to_string()),
    })
    .unwrap_err();
    assert_eq!(error, "rejected: busy");

    let error = response_error(&PromptResponse {
        accepted: true,
        operation_id: None,
        error_message: Some("failed mid-turn".to_string()),
    })
    .unwrap_err();
    assert_eq!(error, "failed mid-turn");

    assert_eq!(
        response_error(&PromptResponse {
            accepted: true,
            operation_id: Some("run-1".to_string()),
            error_message: None,
        }),
        Ok(())
    );
}

#[test]
fn transcript_event_and_text_faces_match_upstream() {
    // Upstream: assistant message_end with a run id records its text.
    assert!(records_completed_text(
        "message_end",
        Some("run-1"),
        "assistant"
    ));
    assert!(!records_completed_text("message_end", None, "assistant"));
    assert!(!records_completed_text(
        "message_end",
        Some("run-1"),
        "user"
    ));
    assert!(!records_completed_text(
        "message_update",
        Some("run-1"),
        "assistant"
    ));

    let completed = vec![
        ("run-1".to_string(), "remote answer".to_string()),
        ("run-2".to_string(), "second".to_string()),
    ];
    assert_eq!(completed_text_for(&completed, "run-1"), "remote answer");
    assert_eq!(completed_text_for(&completed, "missing"), "");

    // Upstream snapshot guard.
    assert_eq!(ensure_initialized_snapshot(true), Ok(()));
    assert_eq!(
        ensure_initialized_snapshot(false).unwrap_err(),
        "Transcript has no initialized snapshot"
    );
}
