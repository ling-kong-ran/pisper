# Pisper TUI

![Pisper TUI welcome screen](https://ling-kong-ran.github.io/pisper/shots/cli.png)

![Pisper TUI chat and live activity](https://ling-kong-ran.github.io/pisper/shots/cli-chat.png)

[Project home](https://ling-kong-ran.github.io/pisper/) · [GitHub repository](https://github.com/ling-kong-ran/pisper)

[中文指南](https://github.com/ling-kong-ran/pisper/blob/release/src-tui/README.md)

## Install the Terminal Command

Requires Node.js 20+. Install Pisper, then run `pisper`:

```bash
npm install -g pisper --progress=true --foreground-scripts
pisper
```

Platform packages use the same registry when an npm mirror is configured. `--progress=true` shows npm download progress, while `--foreground-scripts` shows local signature verification and extraction; add `--loglevel=info` only when request and cache details are needed:

```bash
npm install -g pisper --registry=https://registry.npmmirror.com --progress=true --foreground-scripts
```

After installing the Pisper desktop app, you can alternatively open **Settings → App updates** to install, repair, or uninstall the `pisper` command. Pisper installs it in a current-user directory and manages the matching `PATH` entry without administrator access.

After installing or uninstalling through the desktop app, restart the terminal host. Fully quit and reopen Windows Terminal, or restart the IDE that owns an integrated terminal.

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

## Distribution Updates

Check or update an npm installation of Pisper:

```bash
pisper update --check
pisper update
```

The command uses the configured npm registry. Manage desktop updates under **Settings → App updates**.

## Provider and Optional Web Configuration

After entering the TUI for the first time, run `/provider` and choose a Provider to edit its protocol, Base URL, and API key. The Base URL is prefilled with the Runtime's effective address: the official default when there is no override, or the custom address when one is configured. Leaving API Key blank preserves the saved secret. Use `Up`/`Down` or `Tab` to move between fields, `Left`/`Right` to choose a protocol, `Enter` to save, and `Esc` to return to the Provider list or cancel. The API key uses a separate masked input, whitespace is removed when pasting, and the secret never enters the composer, Slash history, or ordinary Runtime configuration responses. For a known Provider you can skip the picker with `/provider <id>`, e.g. `/provider deepseek`; `/apikey` remains a compatible alias.

For complete visual configuration, run:

```bash
pisper web
```

This opens local Provider settings in your default browser. Keep the Pisper terminal process running while using the browser. **Save Provider settings** saves the current connection; **Set as default Provider** changes the default model for later sessions.

After Web is installed, run `/web` in the TUI to reopen the settings page. If it is not installed yet, exit and run `pisper web` first.

## Startup and Sessions

Run Pisper from a project directory:

```bash
pisper
```

Plain `pisper` always creates an empty conversation. It never restores history automatically. Use `resume` explicitly to open an interactive list containing conversations from every workspace; use the arrow keys to select one, press `Enter` to resume, or press `Esc` to exit:

```bash
pisper resume
```

When no history exists, `pisper resume` reports that condition and exits without creating a conversation. Resuming uses the conversation's saved working directory without rewriting it to the TUI launch directory. After entering a conversation, `/dir <directory>` explicitly changes its working directory; relative paths resolve from the current conversation directory.

Create a new conversation for another workspace:

```bash
pisper --cwd /path/to/project
```

Inspect sidecar, authentication, and capability status:

```bash
pisper doctor
```

The diagnostic output reports the sidecar connection source, TUI launch workspace, sidecar runtime fallback workspace, and the latest matching session workspace so directory inheritance failures are visible.

## Composer and Message Stream

- `Enter`: submit a message. Plain text submitted during an active run is appended immediately through the same Runtime `steer` queue as the desktop app. Messages with attachments wait until the active run finishes so their attachments are preserved.
- `/`: open the combined list of runtime Tools, enabled Skills, and built-in commands.
- `Up` / `Down`: move through Slash, session, model, thinking-level, or file choices.
- `Tab`: complete the highlighted candidate only while the Slash list is open.
- `Esc`: close a picker, clear a Slash draft, or return from Changes to Chat.
- `↑` / `↓`: scroll the conversation one row at a time; `PageUp` / `PageDown`: move eight rows. History loads on demand; the TUI keeps at most 160 messages in memory and evicts back to the latest 80 after an idle period.
- `Home` / `End`, `Left` / `Right`, `Backspace` / `Delete`: edit the composer draft.
- `Ctrl+C`: abort the active or approval-blocked run (if the Agent hangs without responding, the Runtime force-settles the run after a timeout); a second `Ctrl+C` while still running force-quits the TUI; exits only while idle.

The bottom status bar is left-aligned. While a session is running, a single breathing light (a glyph that cycles from dim to bright and back, e.g. `○ ◔ ◑ ◕ ●`) appears at the far left instead of phase labels such as `Thinking` or `Responding`. Execution mode, model, thinking level, and metrics follow, for example `●  [full-access]  gpt-5.6-sol  high  ·  88M  ·  cache 79%`. Queue and approval positions follow the metrics. The values remain isolated per conversation.

The TUI uses terminal truecolor by default and automatically falls back when `TERM` advertises only 256 colors. Set `PISPER_TUI_THEME=ansi256` to force the 256-color palette, or use `PISPER_TUI_THEME=monochrome` / `NO_COLOR=1` for monochrome output. `PISPER_TUI_REDUCED_MOTION=1` disables decorative animation and reveals streamed text immediately; scrolling through history also pauses incremental reveal.

Long or multiline bracketed pastes render as a compact `[Pasted text · …]` token. The complete original text, including line breaks, is still submitted to the Agent. The token can be moved across or deleted as one unit.

## Built-in Commands

| Command | Action |
| :--- | :--- |
| `/init` | Analyze the current project and create or improve `AGENTS.md` at the workspace root. |
| `/new` | Create an empty conversation in the TUI launch workspace; unavailable during a run. |
| `/sessions` | Open the history picker across every workspace; press `Enter` to resume. Unavailable during a run. |
| `/dir <directory>` | Explicitly change the active conversation directory. Relative paths resolve from its current directory; unavailable during a run. |
| `/changes` | Inspect Git or SVN workspace changes. In the changes view, `R` refreshes, `C` commits, `P` pushes Git, and `V` twice reverts; SVN has no push operation. |
| `/changes commit <message>` | Commit current Git/SVN workspace changes with an explicit message. |
| `/chat` | Return to the Chat message stream. |
| `/model` | Open the model picker and switch the active session model; only models from configured Providers are listed, and switching is unavailable during a run. |
| `/thinking` | Refresh and open thinking levels supported by the active model; unsupported/error states explain why and can be retried. Unavailable during a run. |
| `/provider` | Edit a Provider's protocol, effective Base URL, and masked API key; `/provider <id>` targets a Provider directly and `/apikey` remains a compatible alias. |
| `/web` | Open the authenticated local settings page in the default browser using the installed Web frontend. |
| `/compact` | Summarize older context immediately; available only for idle sessions with enough history. |
| `/attach` | Open the workspace file picker. |
| `/mode` | Show the active execution mode and accepted values; it can be changed during a run. |
| `/mode read-only` | Expose low-risk analysis tools only. |
| `/mode full-access` | Allow unrestricted local files, network, and Shell access. |
| `/quit` | Exit the TUI. |

Candidates are sorted by prefix match and local usage frequency. `Tab` completes a command without executing a Tool; press `Enter` to select a built-in command.

`/init` asks the Agent to inspect the project structure, commands, and conventions before writing repository-specific guidance instead of a fixed template. It carefully preserves useful content in an existing `AGENTS.md` and does not modify other project files. The command is unavailable in `read-only` mode and runs with current-user permissions in `full-access` mode. After it completes, use `/new` to start a session that loads the generated project guidance at startup.

When the Agent creates a structured Plan, the TUI updates its items, owners, dependencies, and statuses in place between the transcript and Composer. Narrow terminals keep only the current item, with `Plan completed/total` in the thin top divider.

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

The TUI sends the first Slash token through the structured `requestedToolNames` field. The Agent still generates arguments and performs the call. Slash selection never bypasses Tool settings or the active execution mode; attachments remain limited to the current workspace.

Skill example:

```text
/skill:docs-search find the Tauri updater signing requirements
```

The active Slash list is authoritative for Skill names. The runtime loads the matching `SKILL.md`, scripts, and references on demand. Tools invoked by a Skill follow the same permission chain.

`read-only` does not expose write or Shell capabilities. `full-access` means the user explicitly allows unrestricted access as the current operating-system user.

## Local Development

Building the TUI from source requires Rust 1.88 or newer.

Run from source in the current directory:

```bash
npm run tui:dev
```

Choose another workspace:

```bash
npm run tui:dev -- --cwd /path/to/project
```

Development runs `runtime/sidecar.mjs` by default. To connect an isolated running sidecar:

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
