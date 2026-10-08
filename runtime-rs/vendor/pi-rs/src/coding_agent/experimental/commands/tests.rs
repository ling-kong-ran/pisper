//! Tests for `commands.rs`: pinned to the node oracle
//! (tests/fixtures/experimental_final_oracle/oracle_misc_out.json, `commands`
//! section).

use super::*;

#[test]
fn relay_status_descriptions_match_the_oracle() {
    // Oracle: commands.relay (description + skip pairs).
    let cases: Vec<(Option<&str>, RadiusRelayHostStatus, RelayStatusReport)> = vec![
        (
            None,
            RadiusRelayHostStatus::Connecting,
            RelayStatusReport {
                description: "connecting".to_string(),
                skipped: true,
            },
        ),
        (
            None,
            RadiusRelayHostStatus::Connected,
            RelayStatusReport {
                description: "connected".to_string(),
                skipped: false,
            },
        ),
        (
            Some("connected"),
            RadiusRelayHostStatus::Connected,
            RelayStatusReport {
                description: "connected".to_string(),
                skipped: true,
            },
        ),
        (
            Some("connected"),
            RadiusRelayHostStatus::NotAuthenticated,
            RelayStatusReport {
                description: "not connected; local only".to_string(),
                skipped: false,
            },
        ),
        (
            Some("not connected; local only"),
            RadiusRelayHostStatus::Retrying {
                error: "boom".to_string(),
            },
            RelayStatusReport {
                description: "reconnecting: boom".to_string(),
                skipped: false,
            },
        ),
        (
            None,
            RadiusRelayHostStatus::Retrying {
                error: "net down".to_string(),
            },
            RelayStatusReport {
                description: "reconnecting: net down".to_string(),
                skipped: false,
            },
        ),
    ];
    for (previous, status, expected) in cases {
        assert_eq!(describe_relay_status(previous, &status), expected);
    }
}

#[test]
fn relay_status_printer_follows_the_bookkeeping() {
    // Upstream reportRelayStatus: skip repeats and connecting; remember the
    // last printed description.
    let mut printer = RelayStatusPrinter::default();
    assert_eq!(printer.report(&RadiusRelayHostStatus::Connecting), None);
    assert_eq!(
        printer.report(&RadiusRelayHostStatus::Connected),
        Some("Radius: connected".to_string())
    );
    assert_eq!(printer.report(&RadiusRelayHostStatus::Connected), None);
    assert_eq!(printer.report(&RadiusRelayHostStatus::Connecting), None);
    assert_eq!(
        printer.report(&RadiusRelayHostStatus::NotAuthenticated),
        Some("Radius: not connected; local only".to_string())
    );
    assert_eq!(
        printer.report(&RadiusRelayHostStatus::Retrying {
            error: "socket hung up".to_string(),
        }),
        Some("Radius: reconnecting: socket hung up".to_string())
    );
}

#[test]
fn dispatch_gate_matches_the_oracle() {
    // Upstream runExperimentalCommand: experimental flag + server/client arg.
    assert_eq!(
        run_experimental_command_gate(false, &["server".to_string()]),
        Ok(false)
    );
    assert_eq!(
        run_experimental_command_gate(true, &["other".to_string()]),
        Ok(false)
    );
    assert_eq!(run_experimental_command_gate(true, &[]), Ok(false));
    assert_eq!(
        run_experimental_command_gate(true, &["server".to_string()]),
        Ok(true)
    );
    assert_eq!(
        run_experimental_command_gate(true, &["client".to_string()]),
        Ok(true)
    );
}

#[test]
fn client_output_matches_the_oracle() {
    // Oracle: commands.client.lines / writes.
    use crate::coding_agent::experimental::client::ClientResult;
    use crate::coding_agent::experimental::client::SessionAddress;

    let attached = client_command_output(
        &ClientResult::Attached {
            server_id: "s1".to_string(),
            session_id: "se1".to_string(),
        },
        false,
    );
    assert_eq!(
        attached,
        ClientCommandOutput::Lines(vec!["s1\tse1\tattached".to_string()])
    );

    let prompted_text = client_command_output(
        &ClientResult::Prompted {
            server_id: "s1".to_string(),
            session_id: "se1".to_string(),
            text: "answer".to_string(),
        },
        false,
    );
    assert_eq!(
        prompted_text,
        ClientCommandOutput::Lines(vec!["answer".to_string()])
    );

    let prompted_streamed = client_command_output(
        &ClientResult::Prompted {
            server_id: "s1".to_string(),
            session_id: "se1".to_string(),
            text: "answer".to_string(),
        },
        true,
    );
    assert_eq!(
        prompted_streamed,
        ClientCommandOutput::Writes(vec!["\n".to_string()])
    );

    let list = client_command_output(
        &ClientResult::List {
            sessions: vec![
                SessionAddress {
                    server_id: "b".to_string(),
                    session_id: "2".to_string(),
                },
                SessionAddress {
                    server_id: "a".to_string(),
                    session_id: "9".to_string(),
                },
            ],
        },
        false,
    );
    assert_eq!(
        list,
        ClientCommandOutput::Lines(vec!["b\t2".to_string(), "a\t9".to_string()])
    );
}

#[test]
fn event_stream_filter_and_error_lines_match_upstream() {
    // Upstream onEvent filter and the CLI error face.
    assert!(streams_to_stdout("message_update", Some("text_delta")));
    assert!(!streams_to_stdout("message_update", Some("other")));
    assert!(!streams_to_stdout("message_end", Some("text_delta")));
    assert_eq!(
        cli_error_lines(&["bad flag".to_string(), "another".to_string()]),
        vec!["Error: bad flag".to_string(), "Error: another".to_string()]
    );
    assert_eq!(thrown_error_line("boom"), "Error: boom");
}
