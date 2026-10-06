//! Port of upstream `coding-agent/src/cli/experimental/commands/server.ts`
//! (sha256 63537bdf1aabb6…): the `experimental server` command.

use crate::protocol::protocol::is_server_id;

use super::super::command::{
    string_option, value_option, BuiltInvocation, Command, OptionValue, ParsedCommandInput,
};
use super::super::command_options::{parse_auth, unsupported_options, AuthInput};

/// Upstream `ServerCommand`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerCommand {
    pub command: String,
    pub auth: Option<AuthInput>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub plugin_packages: Option<Vec<String>>,
    pub server_id: Option<String>,
    pub session_dir: Option<String>,
}

fn server_id_option() -> crate::coding_agent::cli::experimental::command::CommandOption {
    value_option(
        "--server-id",
        |value| {
            if is_server_id(value) {
                Ok(OptionValue::ServerId(value.to_string()))
            } else {
                Err(format!(
                    "Invalid --server-id \"{value}\"; expected a lowercase UUIDv4"
                ))
            }
        },
        false,
    )
}

/// Upstream `serverCommand` construction.
pub fn server_command() -> Command {
    let server_id = server_id_option();
    let session_dir = string_option("--session-dir", false);
    let provider = string_option("--provider", false);
    let model = string_option("--model", false);
    let plugin_package = string_option("-e", true);
    let auth_token = super::super::command_options::auth_token_option();
    let auth_token_file = super::super::command_options::auth_token_file_option();

    Command::new("server")
        .option(server_id)
        .option(session_dir)
        .option(provider)
        .option(model)
        .option(plugin_package)
        .option(auth_token)
        .option(auth_token_file)
        .build(move |input: &ParsedCommandInput| {
            let (auth, auth_errors) = parse_auth(input);
            let server_id = input.value("--server-id");
            let session_dir = input.value("--session-dir");
            let provider = input.value("--provider");
            let model = input.value("--model");
            let plugin_packages = input.values("-e");

            let mut errors = auth_errors;
            if provider.is_some() && model.is_none() {
                errors.push("--provider requires --model".to_string());
            }
            errors.extend(unsupported_options("server", input));
            if !errors.is_empty() {
                return Err(errors);
            }

            let text = |value: Option<&OptionValue>| -> Option<String> {
                value.and_then(OptionValue::as_text).map(str::to_string)
            };
            let server_id = match server_id {
                Some(OptionValue::ServerId(id)) => Some(id.clone()),
                _ => None,
            };
            let session_dir = text(session_dir);
            let provider = text(provider);
            let model = text(model);
            let plugin_packages: Option<Vec<String>> = {
                let values: Vec<String> = plugin_packages
                    .iter()
                    .filter_map(|value| value.as_text().map(str::to_string))
                    .collect();
                if values.is_empty() {
                    None
                } else {
                    Some(values)
                }
            };

            Ok(Some(BuiltInvocation::Server(ServerCommand {
                command: "server".to_string(),
                auth,
                provider,
                model,
                plugin_packages,
                server_id,
                session_dir,
            })))
        })
        .action(|invocation, context| {
            if let BuiltInvocation::Server(command) = invocation {
                context.run_server(command);
            }
        })
}
