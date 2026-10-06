//! Port of upstream `coding-agent/src/cli/experimental/commands/client.ts`
//! (sha256 9ffed74a3acd…): the `experimental client` command.

use super::super::command::{
    flag_option, string_option, BuiltInvocation, Command, OptionValue, ParsedCommandInput,
};
use super::super::command_options::{
    connect_option, parse_auth, unsupported_options, AuthInput, TransportAddress,
};

/// Upstream `ClientCommand`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClientCommand {
    pub command: String,
    pub auth: Option<AuthInput>,
    pub connect: Option<TransportAddress>,
    pub session_id: Option<String>,
    pub r#continue: bool,
    pub resume: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub plugin_packages: Option<Vec<String>>,
    pub prompt: Option<String>,
}

/// Upstream `clientCommand` construction.
pub fn client_command() -> Command {
    let connect = connect_option();
    let session_id = string_option("--session-id", false);
    let continue_flag = flag_option("--continue");
    let continue_short = flag_option("-c");
    let resume_flag = flag_option("--resume");
    let resume_short = flag_option("-r");
    let provider = string_option("--provider", false);
    let model = string_option("--model", false);
    let plugin_package = string_option("-e", true);
    let auth_token = super::super::command_options::auth_token_option();
    let auth_token_file = super::super::command_options::auth_token_file_option();

    Command::new("client")
        .option(connect)
        .option(session_id)
        .option(continue_flag)
        .option(continue_short)
        .option(resume_flag)
        .option(resume_short)
        .option(provider)
        .option(model)
        .option(plugin_package)
        .option(auth_token)
        .option(auth_token_file)
        .build(move |input: &ParsedCommandInput| {
            let (auth, auth_errors) = parse_auth(input);
            let connect = input.value("--connect");
            let session_id = input.value("--session-id");
            let should_continue =
                input.value("--continue").is_some() || input.value("-c").is_some();
            let should_resume = input.value("--resume").is_some() || input.value("-r").is_some();
            let provider = input.value("--provider");
            let model = input.value("--model");
            let plugin_packages = input.values("-e");

            let prompt_args: &[String] =
                if input.remaining_args.first().map(String::as_str) == Some("--") {
                    &input.remaining_args[1..]
                } else {
                    &input.remaining_args
                };
            let prompt = if prompt_args.len() == 1
                && (input.remaining_args.first().map(String::as_str) == Some("--")
                    || !prompt_args[0].starts_with('-'))
                && !prompt_args[0].is_empty()
            {
                Some(prompt_args[0].clone())
            } else {
                None
            };

            let mut errors = auth_errors;
            if provider.is_some() && model.is_none() {
                errors.push("--provider requires --model".to_string());
            }
            if [session_id.is_some(), should_continue, should_resume]
                .into_iter()
                .filter(|flag| *flag)
                .count()
                > 1
            {
                errors.push(
                    "--session-id, --continue, and --resume are mutually exclusive".to_string(),
                );
            }
            if !input.remaining_args.is_empty() && prompt.is_none() {
                errors.extend(unsupported_options("client", input));
            }
            if !errors.is_empty() {
                return Err(errors);
            }

            let text = |value: Option<&OptionValue>| -> Option<String> {
                value.and_then(OptionValue::as_text).map(str::to_string)
            };
            let connect = match connect {
                Some(OptionValue::Connect(address)) => Some(address.clone()),
                _ => None,
            };
            let session_id = text(session_id);
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

            Ok(Some(BuiltInvocation::Client(ClientCommand {
                command: "client".to_string(),
                auth,
                connect,
                session_id,
                r#continue: should_continue,
                resume: should_resume,
                provider,
                model,
                plugin_packages,
                prompt,
            })))
        })
        .action(|invocation, context| {
            if let BuiltInvocation::Client(command) = invocation {
                context.run_client(command);
            }
        })
}
