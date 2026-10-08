use crate::agent_core::{Agent, AgentEvent, AgentMessage, SessionWriter};
use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use tokio_util::sync::CancellationToken;

pub const SYSTEM_PROMPT: &str = "\
You are a coding agent working in the user's current directory. \
Use the provided tools to read and edit files and run commands. \
Be concise.";

pub async fn run(agent: &Agent, session: &mut SessionWriter, model_label: &str) -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    println!("pirs ready. model: {model_label}. commands: /clear /model /quit");
    // The M1 per-prompt `prompt(text, callback)` maps onto subscribe-once +
    // `Agent::prompt`: every event renders through the listener, which the
    // agent awaits in order (so rendering is deterministic with the run).
    agent.subscribe(|event: AgentEvent, _signal: CancellationToken| {
        Box::pin(async move {
            crate::cli::render::render_event(&event);
        })
    });
    loop {
        let line = match rl.readline("» ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(&line);

        match line.as_str() {
            "/quit" | "/exit" => break,
            "/clear" => {
                // reset() keeps the replayed prompt/tool baseline, unlike the
                // M1 full transcript clear.
                agent.reset()?;
                println!("context cleared");
                continue;
            }
            "/model" => {
                println!("{model_label}");
                continue;
            }
            cmd if cmd.starts_with('/') => {
                println!("unknown command: {cmd}");
                continue;
            }
            text => {
                let start = agent.state().messages.len();
                let result = agent.prompt(text).await;
                if let Err(e) = result {
                    println!("[error] {e}");
                    continue;
                }
                // Stream failures settle the run successfully with a failed
                // assistant message; the state carries the error text (the
                // M1 AgentError event intent).
                if let Some(error) = agent.state().error_message.clone() {
                    println!("[error] {error}");
                }
                let new_messages: Vec<AgentMessage> = agent.state().messages[start..].to_vec();
                for m in &new_messages {
                    session.append(m)?;
                }
            }
        }
    }
    println!("bye");
    Ok(())
}
