//! Console rendering of auth login flows (upstream `packages/ai/src/cli.ts`
//! `answerPrompt` and the `login` `notify` switch, lines 25-43 and 53-65):
//! stdio prompts, auth-url and device-code printing. Flows stay I/O-free —
//! everything reaches the terminal through the [`AuthInteraction`]
//! implemented here, so tests script interactions instead of a tty.

use std::io::{BufRead, Write};

use futures::future::BoxFuture;

use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, AuthPromptOption,
};

/// Upstream `login`'s notify switch plus `answerPrompt`, rendered to
/// stdout/stdin. `signal()` is `None`: the flow-level cancellation is the
/// fresh token `login` normalizes with (upstream `new AbortController()`),
/// and Ctrl+C simply terminates the process, like upstream readline.
pub struct ConsoleAuthInteraction;

impl ConsoleAuthInteraction {
    fn read_line() -> Result<String, AuthError> {
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) => Err(AuthError::Cancelled),
            Ok(_) => Ok(line.trim_end_matches(['\n', '\r']).to_string()),
            Err(_) => Err(AuthError::Cancelled),
        }
    }
}

impl AuthInteraction for ConsoleAuthInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(async move {
            let AuthPrompt { kind, .. } = prompt;
            match kind {
                // Upstream answerPrompt select arm: numbered options, then the
                // numeric choice; an invalid selection fails the login.
                AuthPromptKind::Select { message, options } => {
                    println!("\n{message}");
                    for (index, option) in options.iter().enumerate() {
                        println!("  {}. {}", index + 1, option.label);
                    }
                    print!("Enter number (1-{}): ", options.len());
                    let _ = std::io::stdout().flush();
                    let input = Self::read_line()?;
                    select_choice(&input, &options)
                        .ok_or_else(|| AuthError::Operation("Invalid selection".to_string()))
                }
                // Upstream default arm: `message (placeholder): `. Secrets
                // echo, like upstream readline (cli.ts answers every
                // non-select prompt the same way).
                AuthPromptKind::Text {
                    message,
                    placeholder,
                }
                | AuthPromptKind::Secret {
                    message,
                    placeholder,
                }
                | AuthPromptKind::ManualCode {
                    message,
                    placeholder,
                } => {
                    print!(
                        "{}{}: ",
                        message,
                        placeholder.map(|p| format!(" ({p})")).unwrap_or_default()
                    );
                    let _ = std::io::stdout().flush();
                    Self::read_line()
                }
            }
        })
    }

    fn notify(&self, event: AuthEvent) {
        print!("{}", render_auth_event(&event));
        let _ = std::io::stdout().flush();
    }
}

/// Upstream answerPrompt's numeric selection: `1`-based, returning the
/// chosen option's id; `None` for anything out of range (the caller fails
/// the login with upstream's "Invalid selection").
fn select_choice(input: &str, options: &[AuthPromptOption]) -> Option<String> {
    let index: usize = input.trim().parse().ok()?;
    options
        .get(index.checked_sub(1)?)
        .map(|option| option.id.clone())
}

/// Upstream `login`'s notify rendering (cli.ts:57-64): auth_url prints the
/// browser URL plus optional instructions, device_code prints the
/// verification URI and the code, info/progress print their message. The
/// leading newlines are upstream's; the trailing newline terminates each
/// print. Upstream's info/progress arm ignores `links`; so does this.
fn render_auth_event(event: &AuthEvent) -> String {
    match event {
        AuthEvent::AuthUrl { url, instructions } => {
            let mut text = format!("\nOpen this URL in your browser:\n{url}\n");
            if let Some(instructions) = instructions {
                text.push_str(instructions);
                text.push('\n');
            }
            text
        }
        AuthEvent::DeviceCode {
            user_code,
            verification_uri,
            ..
        } => format!(
            "\nOpen this URL in your browser:\n{verification_uri}\nEnter code: {user_code}\n"
        ),
        AuthEvent::Info { message, .. } | AuthEvent::Progress { message } => {
            format!("{message}\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, label: &str) -> AuthPromptOption {
        AuthPromptOption {
            id: id.to_string(),
            label: label.to_string(),
            description: None,
        }
    }

    #[test]
    fn select_choice_follows_upstream_numbering() {
        let options = vec![option("one", "One"), option("two", "Two")];
        assert_eq!(select_choice("1", &options).unwrap(), "one");
        assert_eq!(select_choice(" 2\n", &options).unwrap(), "two");
        // Out of range, zero, non-numeric: invalid (upstream "Invalid
        // selection").
        assert!(select_choice("3", &options).is_none());
        assert!(select_choice("0", &options).is_none());
        assert!(select_choice("x", &options).is_none());
    }

    #[test]
    fn auth_events_render_like_upstream_cli() {
        assert_eq!(
            render_auth_event(&AuthEvent::AuthUrl {
                url: "https://auth.example".to_string(),
                instructions: None,
            }),
            "\nOpen this URL in your browser:\nhttps://auth.example\n"
        );
        assert_eq!(
            render_auth_event(&AuthEvent::AuthUrl {
                url: "https://auth.example".to_string(),
                instructions: Some("Paste the code back here.".to_string()),
            }),
            "\nOpen this URL in your browser:\nhttps://auth.example\nPaste the code back here.\n"
        );
        assert_eq!(
            render_auth_event(&AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://dev.example".to_string(),
                interval_seconds: Some(5),
                expires_in_seconds: Some(900),
            }),
            "\nOpen this URL in your browser:\nhttps://dev.example\nEnter code: ABCD-1234\n"
        );
        assert_eq!(
            render_auth_event(&AuthEvent::Info {
                message: "Credentials saved to auth.json".to_string(),
                links: None,
            }),
            "Credentials saved to auth.json\n"
        );
        assert_eq!(
            render_auth_event(&AuthEvent::Progress {
                message: "waiting for the browser".to_string()
            }),
            "waiting for the browser\n"
        );
    }
}
