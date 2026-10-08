use super::Platform;
use pi_rust::coding_agent::utils::shell_config::ShellEnvironment;
use regex::Regex;
use std::sync::OnceLock;

const DENIED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
    "BITBUCKET_TOKEN",
    "NPM_TOKEN",
    "NODE_AUTH_TOKEN",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "DOCKER_AUTH_CONFIG",
    "DATABASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_CLIENT_SECRET",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "SHELLOPTS",
    "BASHOPTS",
    "CDPATH",
    "GLOBIGNORE",
];

pub(crate) fn host_command_environment(
    environment: ShellEnvironment,
    platform: Platform,
) -> ShellEnvironment {
    static CREDENTIAL: OnceLock<Regex> = OnceLock::new();
    let credential = CREDENTIAL.get_or_init(|| Regex::new(r"(?i)(?:^|_)(?:API_?KEY|ACCESS_?TOKEN|AUTH_?TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE_?KEY)$").expect("host shell credential names"));
    environment
        .into_iter()
        .filter(|(name, _)| {
            // Windows environment names are case insensitive at the OS boundary.
            !DENIED.iter().any(|denied| {
                if platform == Platform::Windows {
                    name.eq_ignore_ascii_case(denied)
                } else {
                    name == denied
                }
            }) && !credential.is_match(name)
        })
        .collect()
}
