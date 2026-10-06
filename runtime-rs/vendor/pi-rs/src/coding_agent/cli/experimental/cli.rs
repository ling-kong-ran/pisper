//! Port of upstream `coding-agent/src/cli/experimental/cli.ts` (sha256
//! 65d7601b4946…): the development CLI root (`experimental server|client`).

use super::command::{BuiltInvocation, Command, CommandActionContext, CommandResult};
use super::commands::client::{client_command, ClientCommand};
use super::commands::server::{server_command, ServerCommand};

/// Upstream `CliContext` (`ServerCommandContext & ClientCommandContext`).
pub trait CliContext: CommandActionContext {}

/// The root `experimental` command: requires a `server`/`client` subcommand
/// (upstream's build result `{ ok: false, errors: ["Expected experimental
/// command: server or client"] }`).
pub fn cli() -> Command {
    let experimental = Command::new("experimental").build(|_input| {
        Err(vec![
            "Expected experimental command: server or client".to_string()
        ])
    });
    experimental
        .command(server_command())
        .command(client_command())
}

/// Convenience re-exports mirroring the upstream context handlers.
pub use super::command::Command as ExperimentalCommand;

/// Run `cli().execute(argv, context)` with the action dispatch.
pub fn execute(argv: &[String], context: &dyn CommandActionContext) -> CommandResult {
    cli().execute(argv, context)
}

/// Parse only (upstream `cli.parse`).
pub fn parse(argv: &[String]) -> CommandResult {
    cli().parse(argv)
}

/// The no-op context used by parse-only callers.
pub struct NullContext;

impl CommandActionContext for NullContext {
    fn run_server(&self, _command: &ServerCommand) {}
    fn run_client(&self, _command: &ClientCommand) {}
}

/// Upstream type-level guarantee: the root only ever produces server/client
/// invocations (or the group error).
pub fn is_server(invocation: &BuiltInvocation) -> bool {
    matches!(invocation, BuiltInvocation::Server(_))
}
