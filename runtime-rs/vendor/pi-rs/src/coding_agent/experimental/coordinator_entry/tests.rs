//! Tests for `coordinator_entry.rs` and `plugin.rs` (entry wiring + plugin
//! API surface).

use super::*;
use crate::coding_agent::experimental::plugin::{
    AgentOperationResponse, SlashCommandCompletion, SlashCommandContribution,
    SlashCommandRunResult, AGENT_CONTROLLER, PRESENTATION_PLUGINS, PRESENTATION_UI, SLASH_COMMANDS,
};

#[test]
fn role_arguments_slice_skips_the_entry_arguments() {
    // Upstream `process.argv.slice(2)`.
    let args: Vec<String> = [
        "node",
        "coordinator-entry.ts",
        "--control",
        "/run/control.sock",
    ]
    .iter()
    .map(|part| part.to_string())
    .collect();
    assert_eq!(
        coordinator_arguments(&args),
        vec!["--control".to_string(), "/run/control.sock".to_string()]
    );
    assert!(coordinator_arguments(&["node".to_string(), "entry.ts".to_string()]).is_empty());
}

#[test]
fn role_requirement_uses_the_process_port_role() {
    // Upstream: role !== "coordinator" throws the exact error; the role
    // itself comes from process.rs (`__PI_INTERNAL_SPAWN`), so outside an
    // internal spawn this must refuse with the upstream text.
    let outcome = require_coordinator_role();
    match outcome {
        Ok(()) => {
            // Only reachable when the process was actually launched with the
            // internal coordinator role.
            let role =
                crate::coding_agent::experimental::process::get_internal_process_role().unwrap();
            assert_eq!(
                role,
                Some(crate::coding_agent::experimental::process::InternalProcessRole::Coordinator)
            );
        }
        Err(message) => {
            assert_eq!(
                message,
                "Coordinator entrypoint requires an internal coordinator invocation"
            );
        }
    }
}

#[test]
fn plugin_face_reexports_the_service_surfaces() {
    // Upstream plugin.ts re-exports; ids and local service names are pinned.
    assert_eq!(AGENT_CONTROLLER, "pi.agent-controller");
    assert_eq!(PRESENTATION_PLUGINS, "pi.presentation-plugins");
    assert_eq!(PRESENTATION_UI, "pi.local.presentation-ui");
    assert_eq!(SLASH_COMMANDS, "pi.local.slash-commands");

    // The payload types round-trip as upstream's schemas do.
    let response: AgentOperationResponse =
        serde_json::from_str(r#"{"accepted":true,"operationId":"run-1","error":null}"#).unwrap();
    assert_eq!(response.operation_id.as_deref(), Some("run-1"));

    let contribution = SlashCommandContribution {
        name: "model".to_string(),
        description: "Select a model".to_string(),
    };
    assert_eq!(contribution.name, "model");
    let completion = SlashCommandCompletion {
        name: "model".to_string(),
        description: "Select a model".to_string(),
    };
    assert_eq!(completion.name, contribution.name);
    assert_eq!(completion.description, contribution.description);
    let run_result = SlashCommandRunResult {
        message: Some("Reloaded plugins.".to_string()),
    };
    assert_eq!(run_result.message.as_deref(), Some("Reloaded plugins."));
}
