//! Port of upstream `coding-agent/src/cli/args.ts` (sha256 f8ba813e8acb…):
//! CLI argument parsing and help display.
//!
//! The parser is ported branch-for-branch; the 92-case oracle battery in
//! `tests/fixtures/cli_oracle/oracle.json` (captured from the real upstream source
//! under node) is byte-compared in tests, including help text and error
//! strings.
//!
//! Divergence 1: `printHelp` returns the rendered help text (callers print);
//! upstream chalk bold is rendered plain, which is byte-identical to upstream
//! under a non-TTY.

use crate::ai::types::ModelThinkingLevel;
use crate::coding_agent::extensions::types::{ExtensionFlag, FlagType};

use super::{APP_NAME, CONFIG_DIR_NAME, ENV_AGENT_DIR, ENV_SESSION_DIR};

/// Upstream `Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text,
    Json,
    Rpc,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Mode> {
        match value {
            "text" => Some(Mode::Text),
            "json" => Some(Mode::Json),
            "rpc" => Some(Mode::Rpc),
            _ => None,
        }
    }
}

/// Upstream `TuiMode` (`core/settings-manager.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Regular,
    Fullscreen,
}

/// Upstream diagnostic `{ type: "warning" | "error"; message: string }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticType,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticType {
    Warning,
    Error,
}

/// Value of an unknown long flag (potentially an extension flag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownFlagValue {
    Boolean(bool),
    Text(String),
}

/// Ordered map of unknown flag name → value (upstream `Map`, insertion order
/// preserved so `keys().next().value` — the first unknown flag — matches).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownFlags {
    entries: Vec<(String, UnknownFlagValue)>,
}

impl UnknownFlags {
    pub fn insert(&mut self, name: impl Into<String>, value: UnknownFlagValue) {
        self.entries.push((name.into(), value));
    }

    pub fn get(&self, name: &str) -> Option<&UnknownFlagValue> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Upstream `unknownFlags.keys().next().value` — the first inserted key.
    pub fn first_key(&self) -> Option<&str> {
        self.entries.first().map(|(key, _)| key.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &UnknownFlagValue)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// Upstream `Args`. Optional fields are `None` when absent (upstream
/// `undefined`); [`Args::messages`], [`Args::file_args`],
/// [`Args::unknown_flags`] and [`Args::diagnostics`] always exist.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<Vec<String>>,
    pub thinking: Option<ModelThinkingLevel>,
    pub r#continue: Option<bool>,
    pub resume: Option<bool>,
    pub help: Option<bool>,
    pub version: Option<bool>,
    pub mode: Option<Mode>,
    pub name: Option<String>,
    pub no_session: Option<bool>,
    pub session: Option<String>,
    pub session_id: Option<String>,
    pub fork: Option<String>,
    pub session_dir: Option<String>,
    pub models: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub no_tools: Option<bool>,
    pub no_builtin_tools: Option<bool>,
    pub extensions: Option<Vec<String>>,
    pub no_extensions: Option<bool>,
    pub print: Option<bool>,
    pub export: Option<String>,
    pub no_skills: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub prompt_templates: Option<Vec<String>>,
    pub no_prompt_templates: Option<bool>,
    pub themes: Option<Vec<String>>,
    pub use_theme: Option<String>,
    pub no_themes: Option<bool>,
    pub no_context_files: Option<bool>,
    /// `Some(true)` for a bare `--list-models`, `Some(pattern)` for a search.
    pub list_models: Option<Option<String>>,
    pub offline: Option<bool>,
    pub tui_mode: Option<TuiMode>,
    pub verbose: Option<bool>,
    /// `Some(true)` for `--approve`, `Some(false)` for `--no-approve`.
    pub project_trust_override: Option<bool>,
    pub messages: Vec<String>,
    pub file_args: Vec<String>,
    pub unknown_flags: UnknownFlags,
    pub diagnostics: Vec<Diagnostic>,
}

const VALID_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Upstream `isValidThinkingLevel`.
pub fn is_valid_thinking_level(level: &str) -> Option<ModelThinkingLevel> {
    match level {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        "max" => Some(ModelThinkingLevel::Max),
        _ => None,
    }
}

/// Upstream `normalizeSessionName`.
pub fn normalize_session_name(value: &str) -> Option<String> {
    let name = value.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn split_trimmed_list(value: &str) -> Vec<String> {
    // Upstream v1.0.0: `--models` drops empty patterns after trimming.
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect()
}

fn split_trimmed_nonempty_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Upstream `parseArgs`.
pub fn parse_args(args: &[String]) -> Args {
    let mut result = Args::default();

    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();

        if arg == "--" {
            for positional_arg in &args[i + 1..] {
                if let Some(stripped) = positional_arg.strip_prefix('@') {
                    result.file_args.push(stripped.to_string());
                } else {
                    result.messages.push(positional_arg.clone());
                }
            }
            break;
        } else if arg == "--help" || arg == "-h" {
            result.help = Some(true);
        } else if arg == "--version" || arg == "-v" {
            result.version = Some(true);
        } else if arg == "--mode" {
            let mode = args.get(i + 1);
            match mode {
                None => result.diagnostics.push(Diagnostic {
                    kind: DiagnosticType::Error,
                    message: "--mode requires text, json, or rpc".to_string(),
                }),
                Some(mode) if mode.starts_with('-') => {
                    result.diagnostics.push(Diagnostic {
                        kind: DiagnosticType::Error,
                        message: "--mode requires text, json, or rpc".to_string(),
                    });
                }
                Some(mode) => {
                    i += 1;
                    if mode != "text" && mode != "json" && mode != "rpc" {
                        result.diagnostics.push(Diagnostic {
                            kind: DiagnosticType::Error,
                            message: format!(
                                "Invalid mode \"{mode}\". Valid values: text, json, rpc"
                            ),
                        });
                    } else {
                        result.mode = Mode::parse(mode);
                    }
                }
            }
        } else if arg == "--continue" || arg == "-c" {
            result.r#continue = Some(true);
        } else if arg == "--resume" || arg == "-r" {
            result.resume = Some(true);
        } else if arg == "--provider" && i + 1 < args.len() {
            i += 1;
            result.provider = Some(args[i].clone());
        } else if arg == "--model" && i + 1 < args.len() {
            i += 1;
            result.model = Some(args[i].clone());
        } else if arg == "--api-key" && i + 1 < args.len() {
            i += 1;
            result.api_key = Some(args[i].clone());
        } else if arg == "--system-prompt" && i + 1 < args.len() {
            i += 1;
            result.system_prompt = Some(args[i].clone());
        } else if arg == "--append-system-prompt" && i + 1 < args.len() {
            i += 1;
            result
                .append_system_prompt
                .get_or_insert_with(Vec::new)
                .push(args[i].clone());
        } else if arg == "--name" || arg == "-n" {
            if i + 1 < args.len() {
                i += 1;
                result.name = Some(args[i].clone());
            } else {
                result.diagnostics.push(Diagnostic {
                    kind: DiagnosticType::Error,
                    message: "--name requires a value".to_string(),
                });
            }
        } else if arg == "--no-session" {
            result.no_session = Some(true);
        } else if arg == "--session" && i + 1 < args.len() {
            i += 1;
            result.session = Some(args[i].clone());
        } else if arg == "--session-id" && i + 1 < args.len() {
            i += 1;
            result.session_id = Some(args[i].clone());
        } else if arg == "--fork" && i + 1 < args.len() {
            i += 1;
            result.fork = Some(args[i].clone());
        } else if arg == "--session-dir" && i + 1 < args.len() {
            i += 1;
            result.session_dir = Some(args[i].clone());
        } else if arg == "--models" && i + 1 < args.len() {
            i += 1;
            result.models = Some(split_trimmed_list(&args[i]));
        } else if arg == "--no-tools" || arg == "-nt" {
            result.no_tools = Some(true);
        } else if arg == "--no-builtin-tools" || arg == "-nbt" {
            result.no_builtin_tools = Some(true);
        } else if (arg == "--tools" || arg == "-t") && i + 1 < args.len() {
            i += 1;
            result.tools = Some(split_trimmed_nonempty_list(&args[i]));
        } else if (arg == "--exclude-tools" || arg == "-xt") && i + 1 < args.len() {
            i += 1;
            result.exclude_tools = Some(split_trimmed_nonempty_list(&args[i]));
        } else if arg == "--thinking" && i + 1 < args.len() {
            i += 1;
            match is_valid_thinking_level(&args[i]) {
                Some(level) => result.thinking = Some(level),
                None => result.diagnostics.push(Diagnostic {
                    kind: DiagnosticType::Warning,
                    message: format!(
                        "Invalid thinking level \"{}\". Valid values: {}",
                        args[i],
                        VALID_THINKING_LEVELS.join(", ")
                    ),
                }),
            }
        } else if arg == "--print" || arg == "-p" {
            result.print = Some(true);
            if let Some(next) = args.get(i + 1) {
                let next = next.as_str();
                if !next.starts_with('@') && (!next.starts_with('-') || next.starts_with("---")) {
                    result.messages.push(next.to_string());
                    i += 1;
                }
            }
        } else if arg == "--export" && i + 1 < args.len() {
            i += 1;
            result.export = Some(args[i].clone());
        } else if (arg == "--extension" || arg == "-e") && i + 1 < args.len() {
            i += 1;
            result
                .extensions
                .get_or_insert_with(Vec::new)
                .push(args[i].clone());
        } else if arg == "--no-extensions" || arg == "-ne" {
            result.no_extensions = Some(true);
        } else if arg == "--skill" && i + 1 < args.len() {
            i += 1;
            result
                .skills
                .get_or_insert_with(Vec::new)
                .push(args[i].clone());
        } else if arg == "--prompt-template" && i + 1 < args.len() {
            i += 1;
            result
                .prompt_templates
                .get_or_insert_with(Vec::new)
                .push(args[i].clone());
        } else if arg == "--theme" && i + 1 < args.len() {
            i += 1;
            result
                .themes
                .get_or_insert_with(Vec::new)
                .push(args[i].clone());
        } else if arg == "--use-theme" {
            match args.get(i + 1) {
                Some(theme_name) if !theme_name.starts_with('-') => {
                    result.use_theme = Some(theme_name.clone());
                    i += 1;
                }
                _ => result.diagnostics.push(Diagnostic {
                    kind: DiagnosticType::Error,
                    message: "--use-theme requires a theme name".to_string(),
                }),
            }
        } else if arg == "--no-skills" || arg == "-ns" {
            result.no_skills = Some(true);
        } else if arg == "--no-prompt-templates" || arg == "-np" {
            result.no_prompt_templates = Some(true);
        } else if arg == "--no-themes" {
            result.no_themes = Some(true);
        } else if arg == "--no-context-files" || arg == "-nc" {
            result.no_context_files = Some(true);
        } else if arg == "--list-models" {
            // Check if next arg is a search pattern (not a flag or file arg)
            match args.get(i + 1) {
                Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                    i += 1;
                    result.list_models = Some(Some(next.clone()));
                }
                _ => result.list_models = Some(None),
            }
        } else if arg == "--tui-mode" {
            let mode = args.get(i + 1).map(String::as_str);
            match mode {
                Some("regular") => {
                    result.tui_mode = Some(TuiMode::Regular);
                    i += 1;
                }
                Some("fullscreen") => {
                    result.tui_mode = Some(TuiMode::Fullscreen);
                    i += 1;
                }
                Some(invalid_mode) if !invalid_mode.starts_with('-') => {
                    i += 1;
                    result.diagnostics.push(Diagnostic {
                        kind: DiagnosticType::Error,
                        message: format!(
                            "Invalid TUI mode \"{invalid_mode}\". Valid values: regular, fullscreen"
                        ),
                    });
                }
                _ => {
                    result.diagnostics.push(Diagnostic {
                        kind: DiagnosticType::Error,
                        message: "--tui-mode requires regular or fullscreen".to_string(),
                    });
                }
            }
        } else if arg == "--verbose" {
            result.verbose = Some(true);
        } else if arg == "--approve" || arg == "-a" {
            result.project_trust_override = Some(true);
        } else if arg == "--no-approve" || arg == "-na" {
            result.project_trust_override = Some(false);
        } else if arg == "--offline" {
            result.offline = Some(true);
        } else if let Some(stripped) = arg.strip_prefix('@') {
            result.file_args.push(stripped.to_string()); // Remove @ prefix
        } else if let Some(rest) = arg.strip_prefix("--") {
            if let Some(eq_index) = rest.find('=') {
                result.unknown_flags.insert(
                    &rest[..eq_index],
                    UnknownFlagValue::Text(rest[eq_index + 1..].to_string()),
                );
            } else {
                let flag_name = rest;
                match args.get(i + 1) {
                    Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                        result
                            .unknown_flags
                            .insert(flag_name, UnknownFlagValue::Text(next.clone()));
                        i += 1;
                    }
                    _ => {
                        result
                            .unknown_flags
                            .insert(flag_name, UnknownFlagValue::Boolean(true));
                    }
                }
            }
        } else if arg.starts_with('-') {
            result.diagnostics.push(Diagnostic {
                kind: DiagnosticType::Error,
                message: format!("Unknown option: {arg}"),
            });
        } else {
            result.messages.push(arg.to_string());
        }

        i += 1;
    }

    result
}

/// JS `String.prototype.padEnd(width)` with spaces.
fn pad_end(value: &str, width: usize) -> String {
    let mut out = String::from(value);
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Upstream `printHelp` extension-flags section.
fn extension_flags_text(extension_flags: &[ExtensionFlag]) -> String {
    if extension_flags.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = extension_flags
        .iter()
        .map(|flag| {
            let value = if flag.flag_type == FlagType::String {
                " <value>"
            } else {
                ""
            };
            let description = flag
                .description
                .clone()
                .unwrap_or_else(|| format!("Registered by {}", flag.extension_path));
            format!(
                "{}{}",
                pad_end(&format!("  --{}{value}", flag.name), 30),
                description
            )
        })
        .collect();
    format!("\nExtension CLI Flags:\n{}\n", lines.join("\n"))
}

/// Upstream `printHelp` (divergence 1: returns the text; bold rendered plain).
pub fn print_help(extension_flags: &[ExtensionFlag]) -> String {
    let extension_flags_text = extension_flags_text(extension_flags);
    format!(
        "{APP_NAME} - AI coding assistant with read, bash, edit, write tools

Usage:
  {APP_NAME} [options] [--] [@files...] [messages...]

Commands:
  {APP_NAME} install <source> [-l]     Install extension source and add to settings
  {APP_NAME} remove <source> [-l]      Remove extension source from settings
  {APP_NAME} uninstall <source> [-l]   Alias for remove
  {APP_NAME} update [source|self|pi]   Update pi, extensions, or model catalogs
  {APP_NAME} list                      List installed extensions from settings
  {APP_NAME} config [-l]               Open TUI to enable/disable package resources (Tab switches scope)
  {APP_NAME} auth <command>            Print credentials or check provider readiness
  {APP_NAME} mcp <command>             Check MCP servers, sign in to or out of OAuth servers
  {APP_NAME} <command> --help          Show help for install/remove/uninstall/update/list/config/auth/mcp

Options:
  --provider <name>              Provider to search for --model (requires --model)
  --model <pattern>              Model pattern or ID (supports \"provider/id\" and optional \":<thinking>\")
  --api-key <key>                API key (defaults to env vars)
  --system-prompt <text>         System prompt (default: coding assistant prompt)
  --append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)
  --mode <mode>                  Output mode: text (default), json, or rpc
  --print, -p                    Non-interactive mode: process prompt and exit
  --continue, -c                 Continue previous session
  --resume, -r                   Select a session to resume
  --session <path|id>            Use specific session file or partial UUID
  --session-id <id>              Use exact project session ID, creating it if missing
  --fork <path|id>               Fork specific session file or partial UUID into a new session
  --session-dir <dir>            Directory for session storage and lookup
  --no-session                   Don't save session (ephemeral)
  --name, -n <name>              Set session display name
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                                 Supports globs (anthropic/*, *sonnet*) and fuzzy matching
  --no-tools, -nt                Disable all tools by default (built-in and extension)
  --no-builtin-tools, -nbt       Disable built-in tools by default but keep extension/custom tools enabled
  --tools, -t <tools>            Comma-separated allowlist of tool names to enable
                                 Applies to built-in, extension, and custom tools
  --exclude-tools, -xt <tools>   Comma-separated denylist of tool names to disable
                                 Applies to built-in, extension, and custom tools
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh, max
  --extension, -e <path>         Load an extension file or builtin:<name> (can be used multiple times)
  --no-extensions, -ne           Disable extension discovery and built-in extensions (explicit -e paths still work)
  --skill <path>                 Load a skill file or directory (can be used multiple times)
  --no-skills, -ns               Disable skills discovery and loading
  --prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
  --no-prompt-templates, -np     Disable prompt template discovery and loading
  --theme <path>                 Load a theme file or directory (can be used multiple times)
  --use-theme <name[/name]>      Set the initial interactive theme for this run
  --no-themes                    Disable theme discovery and loading
  --no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading
  --export <file>                Export session file to HTML and exit
  --list-models [search]         List available models (with optional fuzzy search)
  --verbose                      Force verbose startup (overrides quietStartup setting)
  --tui-mode <mode>              TUI mode: fullscreen (default) or regular
  --approve, -a                  Trust project-local files for this run
  --no-approve, -na              Ignore project-local files for this run
  --offline                      Disable startup network operations (same as PI_OFFLINE=1)
  --                             End option parsing; treat remaining arguments as messages/files
  --help, -h                     Show this help
  --version, -v                  Show version number

Extensions can register additional flags (e.g., --plan from plan-mode extension).{extension_flags_text}

Examples:
  # Print a provider API key for an external client
  {APP_NAME} auth print-api-key --provider openai

  # Print an OAuth bearer token for an external client (refreshes if expired)
  {APP_NAME} auth print-bearer-token --provider openai-codex

  # Interactive mode
  {APP_NAME}

  # Interactive mode with initial prompt
  {APP_NAME} \"List all .ts files in src/\"

  # Include files in initial message
  {APP_NAME} @prompt.md @image.png \"What color is the sky?\"

  # Non-interactive mode (process and exit)
  {APP_NAME} -p \"List all .ts files in src/\"

  # Prompt beginning with a dash
  {APP_NAME} -p -- \"- Summarize these points\"

  # Multiple messages (interactive)
  {APP_NAME} \"Read package.json\" \"What dependencies do we have?\"

  # Continue previous session
  {APP_NAME} --continue \"What did we discuss?\"

  # Start a named session
  {APP_NAME} --name \"Refactor auth module\"

  # Use different model
  {APP_NAME} --provider openai --model gpt-4o-mini \"Help me refactor this code\"

  # Use model with provider prefix (no --provider needed)
  {APP_NAME} --model openai/gpt-4o \"Help me refactor this code\"

  # Use model with thinking level shorthand
  {APP_NAME} --model sonnet:high \"Solve this complex problem\"

  # Limit model cycling to specific models
  {APP_NAME} --models claude-sonnet,claude-haiku,gpt-4o

  # Limit to a specific provider with glob pattern
  {APP_NAME} --models \"github-copilot/*\"

  # Cycle models with fixed thinking levels
  {APP_NAME} --models sonnet:high,haiku:low

  # Start with a specific thinking level
  {APP_NAME} --thinking high \"Solve this complex problem\"

  # Read-only mode (no file modifications possible)
  {APP_NAME} --tools read,grep,find,ls -p \"Review the code in src/\"

  # Disable one tool while keeping the rest available
  {APP_NAME} --exclude-tools ask_question

  # Export a session file to HTML
  {APP_NAME} --export ~/{CONFIG_DIR_NAME}/agent/sessions/--path--/session.jsonl
  {APP_NAME} --export session.jsonl output.html

Environment Variables:
  ANTHROPIC_AUTH_TOKEN             - Anthropic bearer auth token
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ANTHROPIC_OAUTH_TOKEN            - Anthropic OAuth token (alternative to API key)
  ANT_LING_API_KEY                 - Ant Ling API key
  OPENAI_API_KEY                   - OpenAI GPT API key
  AZURE_OPENAI_API_KEY             - Azure OpenAI API key
  AZURE_OPENAI_BASE_URL            - Azure OpenAI/Cognitive Services base URL (e.g. https://{{resource}}.openai.azure.com)
  AZURE_OPENAI_RESOURCE_NAME       - Azure OpenAI resource name (alternative to base URL)
  AZURE_OPENAI_API_VERSION         - Azure OpenAI API version (default: v1)
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP - Azure OpenAI model=deployment map (comma-separated)
  DEEPSEEK_API_KEY                 - DeepSeek API key
  NVIDIA_API_KEY                   - NVIDIA NIM API key
  GEMINI_API_KEY                   - Google Gemini API key
  GROQ_API_KEY                     - Groq API key
  CEREBRAS_API_KEY                 - Cerebras API key
  XAI_API_KEY                      - xAI Grok API key
  FIREWORKS_API_KEY                - Fireworks API key
  TOGETHER_API_KEY                 - Together AI API key
  BASETEN_API_KEY                  - Baseten API key
  OPENROUTER_API_KEY               - OpenRouter API key
  AI_GATEWAY_API_KEY               - Vercel AI Gateway API key
  ZAI_API_KEY                      - ZAI Coding Plan API key (Global)
  ZAI_CODING_CN_API_KEY            - ZAI Coding Plan API key (China)
  MISTRAL_API_KEY                  - Mistral API key
  MINIMAX_API_KEY                  - MiniMax API key
  MOONSHOT_API_KEY                 - Moonshot AI API key
  OPENCODE_API_KEY                 - OpenCode Zen/OpenCode Go API key
  KIMI_API_KEY                     - Kimi For Coding API key
  META_API_KEY                     - Meta Model API key
  CLOUDFLARE_API_KEY               - Cloudflare API token (Workers AI and AI Gateway)
  CLOUDFLARE_ACCOUNT_ID            - Cloudflare account id (required for both)
  CLOUDFLARE_GATEWAY_ID            - Cloudflare AI Gateway slug (required for AI Gateway)
  QWEN_TOKEN_PLAN_API_KEY          - Qwen Token Plan API key (international region)
  QWEN_TOKEN_PLAN_CN_API_KEY       - Qwen Token Plan API key (China region)
  XIAOMI_API_KEY                   - Xiaomi MiMo API key (api.xiaomimimo.com billing)
  XIAOMI_TOKEN_PLAN_CN_API_KEY     - Xiaomi MiMo Token Plan API key (China region)
  XIAOMI_TOKEN_PLAN_AMS_API_KEY    - Xiaomi MiMo Token Plan API key (Amsterdam region)
  XIAOMI_TOKEN_PLAN_SGP_API_KEY    - Xiaomi MiMo Token Plan API key (Singapore region)
  AWS_PROFILE                      - AWS profile for Amazon Bedrock
  AWS_ACCESS_KEY_ID                - AWS access key for Amazon Bedrock
  AWS_SECRET_ACCESS_KEY            - AWS secret key for Amazon Bedrock
  AWS_BEARER_TOKEN_BEDROCK         - Bedrock API key (bearer token)
  AWS_REGION                       - AWS region for Amazon Bedrock (e.g., us-east-1)
  {:<32} - Config directory (default: ~/{CONFIG_DIR_NAME}/agent)
  {:<32} - Session storage directory (overridden by --session-dir)
  PI_PACKAGE_DIR                   - Override package directory (for Nix/Guix store paths)
  PI_OFFLINE                       - Disable startup network operations when set to 1/true/yes
  PI_TELEMETRY                     - Override install telemetry when set to 1/true/yes or 0/false/no
  PI_SHARE_VIEWER_URL              - Base URL for /share command (default: https://pi.dev/session/)

Built-in Tool Names:
  read       - Read file contents
  bash       - Execute bash commands
  powershell - Execute PowerShell commands on Windows
  edit       - Edit files with find/replace
  write      - Write files (creates/overwrites)
  grep       - Search file contents (read-only, off by default)
  find       - Find files by glob pattern (read-only, off by default)
  ls         - List directory contents (read-only, off by default)
",
        pad_end(ENV_AGENT_DIR, 32),
        pad_end(ENV_SESSION_DIR, 32),
    )
}

/// Console rendering of the help (the upstream `console.log` side effect).
pub fn print_help_to_console(extension_flags: &[ExtensionFlag]) {
    println!("{}", print_help(extension_flags));
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
