# Pisper TUI

Pisper's Rust + Ratatui terminal client. It reuses the desktop app's Node SEA sidecar, Agent runtime, sessions, Tools, Skills, MCP, execution modes, and approval chain instead of shipping a second Agent runtime.

![Pisper TUI welcome screen](../docs/shots/cli.png)

[中文指南](./README.md)

## Install the Terminal Command

After installing the Pisper desktop app, open **Settings → Terminal** to install, repair, or uninstall the `pisper` command. Pisper installs it in a current-user directory and manages the matching `PATH` entry without administrator access.

Restart the terminal host after installing or uninstalling. Fully quit and reopen Windows Terminal, or restart the IDE that owns an integrated terminal.

To build the complete distribution from source:

```bash
npm run sidecar:sea
npm run tui:package
```

The output is written to:

```text
release/tui/pisper-<version>-<platform>-<arch>/
├── pisper[.exe]
├── pisper-sidecar[.exe]
└── sidecar-runtime/
```

## Startup and Sessions

Run Pisper from a project directory:

```bash
pisper
```

Plain `pisper` always creates an empty conversation. It never restores history automatically. Use `resume` explicitly to restore the most recent conversation for the current workspace:

```bash
pisper resume
pisper resume --cwd /path/to/project
```

Create a new conversation for another workspace:

```bash
pisper --cwd /path/to/project
```

Inspect sidecar, authentication, and capability status:

```bash
pisper doctor
```

The TUI prefers an active desktop sidecar. If one is unavailable, a packaged TUI starts the SEA sidecar beside it. Exiting closes only a sidecar started by that TUI process.

## Composer and Message Stream

- `Enter`: submit a message. Messages submitted during an active run enter a FIFO queue and are sent in order after the run finishes normally.
- `/`: open the combined list of runtime Tools, enabled Skills, and built-in commands.
- `Up` / `Down`: move through Slash, session, model, thinking-level, or file choices.
- `Tab`: complete the highlighted candidate only while the Slash list is open.
- `Esc`: close a picker, clear a Slash draft, or return from Events to Chat.
- `PageUp` / `PageDown`: scroll the conversation.
- `Home` / `End`, `Left` / `Right`, `Backspace` / `Delete`: edit the composer draft.
- `Ctrl+C`: abort the active or approval-blocked run; exit only while idle.

Long or multiline bracketed pastes render as a compact `[Pasted text · …]` token. The complete original text, including line breaks, is still submitted to the Agent. The token can be moved across or deleted as one unit.

When a model provides reasoning tokens, the TUI renders live `THINK` content before the response. It never invents reasoning when the provider does not supply it. Tools, Subagents, and response text appear inline; `/events` opens the complete event ledger.

## Built-in Commands

| Command | Action |
| :--- | :--- |
| `/new` | Create an empty conversation; unavailable during a run. |
| `/sessions` | Open the conversation picker; unavailable during a run. |
| `/events` | Open the event ledger for the current TUI process. |
| `/chat` | Return to the Chat message stream. |
| `/model` | Open the model picker and switch the active session model; unavailable during a run. |
| `/thinking` | Open the thinking levels supported by the active model; unavailable during a run. |
| `/attach` | Open the workspace file picker. |
| `/mode` | Show the active execution mode and accepted values. |
| `/mode read-only` | Expose low-risk analysis tools only. |
| `/mode workspace` | Read directly; approve file writes and every Shell command. |
| `/mode full-access` | Allow unrestricted local files, network, and Shell access. |
| `/quit` | Exit the TUI. |

Candidates are sorted by prefix match and local usage frequency. `Tab` completes a command without executing a Tool; press `Enter` to select a built-in command.

## Attachments

Open the attachment picker with any of these controls:

- type `+` while the composer is empty
- press `Ctrl+O`
- run `/attach`

File picker controls:

- `Up` / `Down`: select a file or directory.
- `Enter` / `Right`: enter a directory or add a file.
- `Left`: move to the parent without leaving the active workspace.
- `Delete`: remove the selected attachment.
- `Esc`: close the picker while preserving the composer draft.

The desktop attachment boundary is shared by the TUI: up to 8 files, 10 MiB per file, and 20 MiB total. Files must be workspace-local images, UTF-8 text/code, or supported documents. Images become visual input only when the active model explicitly supports images.

## Tools and Skills

After typing `/`, `T` marks runtime Tools and `S` marks enabled, model-invocable Skills.

Tool examples:

```text
/read README.md
/bash npm test
/web_search Pisper latest release
/mcp_pencil_get_editor_state_f9837b9b inspect the current Pencil canvas
```

The TUI sends the first Slash token through the structured `requestedToolNames` field. The Agent still generates arguments and performs the call. Slash selection never bypasses Tool settings, execution mode, workspace boundaries, or approval.

Skill example:

```text
/skill:docs-search find the Tauri updater signing requirements
```

The active Slash list is authoritative for Skill names. The runtime loads the matching `SKILL.md`, scripts, and references on demand. Tools invoked by a Skill follow the same permission chain.

## Approvals

In `workspace` mode, file writes and every Shell command require per-request approval. The approval panel temporarily replaces the composer:

- `Y`: Allow once.
- `N`: deny.
- `Esc`: deny.
- `Ctrl+C`: deny the pending request and abort the active run.

`read-only` does not expose write capabilities. `full-access` means the user explicitly allows unrestricted access; it is not a security sandbox.

## Local Development

Run from source in the current directory:

```bash
npm run tui:dev
```

Choose another workspace:

```bash
npm run tui:dev -- --cwd /path/to/project
```

Development runs `server/sidecar.mjs` by default. To connect an isolated running sidecar:

```text
PISPER_TUI_URL=http://127.0.0.1:<port>
PISPER_TUI_TOKEN=<desktop token>
```

Or specify a sidecar executable and runtime:

```text
PISPER_SIDECAR_PATH=/path/to/pisper-sidecar
PISPER_APP_ROOT=/path/to/sidecar-runtime
```

## Verification

```bash
npm run tui:test
npm run tui:check
cargo clippy --manifest-path src-tui/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path src-tui/Cargo.toml --release
```
