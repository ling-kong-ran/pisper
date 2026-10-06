//! Port of upstream `coding-agent/src/cli/experimental/command.ts` (sha256
//! e5fed217d6fa…): the `Command` registration/parse/execute machinery for the
//! development CLI.
//!
//! Upstream is generic over per-command invocation types; the port stores
//! built invocations in the [`BuiltInvocation`] enum (the development CLI's
//! grammar is a closed set: the `experimental` group, `server`, `client`),
//! preserving the parse semantics, error strings and dispatch order exactly.

use std::collections::HashMap;

use super::commands::client::ClientCommand;
use super::commands::server::ServerCommand;

/// Upstream `CommandOptionParseResult`, erased to the value kinds the
/// development CLI's options produce.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    Text(String),
    Connect(super::command_options::TransportAddress),
    Auth(super::command_options::AuthInput),
    ServerId(String),
    Flag(bool),
}

impl OptionValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            OptionValue::Text(value) => Some(value),
            _ => None,
        }
    }
}

/// Upstream `NamedCommandInvocation` (marker: a built invocation carries its
/// command name).
#[derive(Debug, Clone, PartialEq)]
pub enum BuiltInvocation {
    Group(String),
    Server(ServerCommand),
    Client(ClientCommand),
}

impl BuiltInvocation {
    pub fn command_name(&self) -> &str {
        match self {
            BuiltInvocation::Group(name) => name,
            BuiltInvocation::Server(command) => &command.command,
            BuiltInvocation::Client(command) => &command.command,
        }
    }
}

/// Upstream `CommandParseResult` / `CommandBuildResult` /
/// `CommandExecutionResult`.
pub type CommandResult = Result<BuiltInvocation, Vec<String>>;

/// Upstream `CommandOption`.
#[derive(Clone)]
pub struct CommandOption {
    pub name: String,
    pub flag: bool,
    pub repeatable: bool,
    pub parse: fn(&str) -> Result<OptionValue, String>,
}

/// Upstream `valueOption`.
pub fn value_option(
    name: &str,
    parse: fn(&str) -> Result<OptionValue, String>,
    repeatable: bool,
) -> CommandOption {
    CommandOption {
        name: name.to_string(),
        flag: false,
        repeatable,
        parse,
    }
}

/// Upstream `stringOption`.
pub fn string_option(name: &str, repeatable: bool) -> CommandOption {
    value_option(
        name,
        |value| Ok(OptionValue::Text(value.to_string())),
        repeatable,
    )
}

/// Upstream `flagOption`.
pub fn flag_option(name: &str) -> CommandOption {
    CommandOption {
        name: name.to_string(),
        flag: true,
        repeatable: false,
        parse: |_| Ok(OptionValue::Flag(true)),
    }
}

/// Upstream `ParsedCommandInput`.
#[derive(Debug, Clone, Default)]
pub struct ParsedCommandInput {
    pub remaining_args: Vec<String>,
    pub(crate) values: HashMap<String, Vec<OptionValue>>,
    pub(crate) errors: Vec<String>,
}

impl ParsedCommandInput {
    /// Upstream `input.value(option)`.
    pub fn value(&self, name: &str) -> Option<&OptionValue> {
        self.values.get(name).and_then(|values| values.first())
    }

    /// Upstream `input.values(option)`.
    pub fn values(&self, name: &str) -> &[OptionValue] {
        self.values.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    fn push(&mut self, name: &str, value: OptionValue) {
        self.values.entry(name.to_string()).or_default().push(value);
    }
}

/// Upstream `CommandAction` context: what an action may do with its parsed
/// command (run the server/client, per the development CLI).
pub trait CommandActionContext {
    fn run_server(&self, command: &ServerCommand);
    fn run_client(&self, command: &ClientCommand);
}

type Builder =
    Box<dyn Fn(&ParsedCommandInput) -> Result<Option<BuiltInvocation>, Vec<String>> + Send + Sync>;
type Action = Box<dyn Fn(&BuiltInvocation, &dyn CommandActionContext) + Send + Sync>;

/// Upstream `Command`.
#[derive(Default)]
pub struct Command {
    pub name: String,
    options: Vec<CommandOption>,
    subcommands: Vec<Command>,
    builder: Option<Builder>,
    action: Option<Action>,
}

impl Command {
    /// Upstream `new Command(name)`.
    pub fn new(name: &str) -> Command {
        Command {
            name: name.to_string(),
            ..Command::default()
        }
    }

    /// Upstream `.option(option)`.
    pub fn option(mut self, option: CommandOption) -> Self {
        if self
            .options
            .iter()
            .any(|existing| existing.name == option.name)
        {
            panic!(
                "Option {} is already registered for {}",
                option.name, self.name
            );
        }
        self.options.push(option);
        self
    }

    /// Upstream `.build(builder)`; `None` results stand for the upstream
    /// `{ ok: false }` build outcome that cannot fail without errors.
    pub fn build(
        mut self,
        builder: impl Fn(&ParsedCommandInput) -> Result<Option<BuiltInvocation>, Vec<String>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.builder = Some(Box::new(builder));
        self
    }

    /// Upstream `.action(action)`.
    pub fn action(
        mut self,
        action: impl Fn(&BuiltInvocation, &dyn CommandActionContext) + Send + Sync + 'static,
    ) -> Self {
        self.action = Some(Box::new(action));
        self
    }

    /// Upstream `.command(subcommand)`.
    pub fn command(mut self, command: Command) -> Self {
        if self
            .subcommands
            .iter()
            .any(|existing| existing.name == command.name)
        {
            panic!("Command {} is already registered", command.name);
        }
        self.subcommands.push(command);
        self
    }

    /// Upstream `.parse(argv)`.
    pub fn parse(&self, argv: &[String]) -> CommandResult {
        if let Some(selected) = self.select(argv) {
            return selected.command.parse(selected.argv);
        }
        self.parse_own(argv)
    }

    /// Upstream `.execute(argv, context)`.
    pub fn execute(&self, argv: &[String], context: &dyn CommandActionContext) -> CommandResult {
        if let Some(selected) = self.select(argv) {
            return selected.command.execute(selected.argv, context);
        }

        let parsed = self.parse_own(argv)?;
        let Some(action) = &self.action else {
            panic!("Command {} does not define an action", self.name);
        };
        action(&parsed, context);
        Ok(parsed)
    }

    fn select<'a>(&'a self, argv: &'a [String]) -> Option<Selected<'a>> {
        let candidate = argv.first()?;
        let command = self
            .subcommands
            .iter()
            .find(|command| &command.name == candidate)?;
        Some(Selected {
            command,
            argv: &argv[1..],
        })
    }

    fn parse_own(&self, argv: &[String]) -> CommandResult {
        let Some(builder) = &self.builder else {
            panic!("Command {} does not define a builder", self.name);
        };
        let parsed = self.parse_options(argv);
        let built = builder(&parsed);
        // Upstream: errors = [...parsed.errors, ...(built.ok ? [] : built.errors)];
        // the combined list decides the outcome, even when the builder "succeeded".
        let mut errors = parsed.errors.clone();
        let built_command = match built {
            Ok(Some(command)) => Some(command),
            Ok(None) => None,
            Err(builder_errors) => {
                errors.extend(builder_errors);
                None
            }
        };
        if !errors.is_empty() {
            return Err(errors);
        }
        match built_command {
            Some(command) => Ok(command),
            None => panic!("Command {} failed without an error", self.name),
        }
    }

    /// Test-only view of option parsing (upstream tests the grammar through
    /// `parse`, but the `--` remaining-args passthrough needs direct access).
    pub fn parse_options_public(&self, argv: &[String]) -> ParsedCommandInput {
        self.parse_options(argv)
    }

    fn parse_options(&self, argv: &[String]) -> ParsedCommandInput {
        let mut parsed = ParsedCommandInput::default();
        let mut index = 0usize;
        while index < argv.len() {
            let argument = &argv[index];
            if argument == "--" {
                parsed.remaining_args.extend(argv[index..].iter().cloned());
                break;
            }

            let equals = argument.find('=');
            let (name, inline_value) = match equals {
                Some(equals) => (&argument[..equals], Some(&argument[equals + 1..])),
                None => (argument.as_str(), None),
            };
            let Some(option) = self.options.iter().find(|option| option.name == name) else {
                parsed.remaining_args.extend(argv[index..].iter().cloned());
                break;
            };

            let value: String;
            if option.flag {
                if inline_value.is_some() {
                    parsed.errors.push(format!("{name} does not take a value"));
                    index += 1;
                    continue;
                }
                value = String::new();
            } else {
                let mut candidate = inline_value.map(str::to_string);
                if candidate.is_none() {
                    if let Some(next) = argv.get(index + 1) {
                        if !next.starts_with('-') {
                            candidate = Some(next.clone());
                            index += 1;
                        }
                    }
                }
                match candidate {
                    Some(candidate) if !candidate.is_empty() => value = candidate,
                    _ => {
                        parsed.errors.push(format!("{name} requires a value"));
                        index += 1;
                        continue;
                    }
                }
            }

            if !parsed.values(name).is_empty() && !option.repeatable {
                parsed
                    .errors
                    .push(format!("{name} may only be specified once"));
                index += 1;
                continue;
            }
            match (option.parse)(&value) {
                Ok(parsed_value) => parsed.push(name, parsed_value),
                Err(error) => parsed.errors.push(error),
            }
            index += 1;
        }
        parsed
    }
}

struct Selected<'a> {
    command: &'a Command,
    argv: &'a [String],
}
