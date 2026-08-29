# Pisper TUI Command Reference

[Project home](https://ling-kong-ran.github.io/pisper/) · [GitHub repository](https://github.com/ling-kong-ran/pisper) · [中文](./README.md)

Argument notation: `<argument>` is required; `[argument]` is optional.

## CLI Commands

| Command | Explanation |
| :--- | :--- |
| `pisper` | Start the TUI in the current directory with a new empty conversation. History is not resumed automatically. |
| `pisper --cwd <directory>` | Start the TUI with a new empty conversation in the specified directory. |
| `pisper resume` | Open the conversation picker across all workspaces. If no history exists, report it and exit. |
| `pisper resume --cwd <directory>` | Open the conversation picker from a specified launch directory. A resumed conversation keeps its own saved working directory. |
| `pisper doctor` | Diagnose Sidecar startup, Runtime authentication, capability catalogs, and the latest conversation directory. |
| `pisper doctor --cwd <directory>` | Run diagnostics with the specified launch workspace. |
| `pisper web` | Install or use the Web frontend and open its authenticated local settings page in the default browser. Keep the process running while using the page. |
| `pisper web --cwd <directory>` | Start the Runtime for a specified directory and open the Web settings page. |
| `pisper help [COMMAND]` | Show global help. `pisper help web` shows help for the Web subcommand. |
| `pisper -h` / `pisper --help` | Show command help without starting the Runtime. |
| `pisper -V` / `pisper --version` | Print the installed TUI version. |
| `pisper update --check` | Check an npm-installed Pisper for updates without installing them. |
| `pisper update` | Update an npm-installed Pisper through the active npm registry and verify the components again. |

## Environment Variables

| Variable | Description |
| :--- | :--- |
| `PISPER_TUI_MOUSE=1` | Enable mouse wheel scrolling in the transcript; note that mouse capture replaces the terminal's native text selection. |
| `PISPER_TUI_REDUCED_MOTION=1` | Disable the typewriter reveal and status animations (streaming text appears immediately); useful on low-end terminals and SSH sessions. |

## Slash Commands

| Command | Explanation |
| :--- | :--- |
| `/init` | Analyze the current project and create or improve `AGENTS.md` at the project root. Existing useful guidance is preserved, and no other project files are changed. |
| `/new` | Create an empty conversation in the TUI launch directory. Unavailable during an Agent run. |
| `/sessions` | Open the conversation picker across all directories. Switching is unavailable during an Agent run. |
| `/dir <directory>` | Change the active conversation's working directory. Relative paths resolve from its current directory. Unavailable during an Agent run. |
| `/changes` | Open Git or SVN changes. In that view, `R` refreshes, `C` commits, `P` pushes Git, and pressing `V` twice reverts. SVN has no Push operation. |
| `/changes commit <message>` | Commit the current Git or SVN changes with an explicit message. |
| `/chat` | Return to the Chat message stream from another view. |
| `/model` | Open the model picker and switch the active conversation model. Only configured Provider models are listed. Unavailable during an Agent run. |
| `/thinking` | Refresh and select thinking levels supported by the active model. Unavailable during an Agent run. |
| `/provider [id]` | Edit a Provider's protocol, effective Base URL, and masked API key. Pass `id` to open a known Provider directly. |
| `/apikey [id]` | Compatibility alias for `/provider`. |
| `/web` | Open the authenticated local settings page in the default browser using the installed Web frontend. |
| `/compact` | Summarize older context immediately. Available only for idle conversations with enough history. |
| `/attach` | Open the attachment picker rooted at the active conversation directory. |
| `/mode` | Show the active execution mode and accepted values. The mode can also change during an Agent run. |
| `/mode approval-required` | Ask before writes, Shell commands, and high-risk tools. |
| `/mode workspace-write` | Auto-approve workspace edits and routine commands. |
| `/mode full-access` | Allow local files, network access, and Shell commands as the current operating-system user. |
| `/quit` | Exit the TUI. |

## Tool and Skill Commands

| Form | Explanation |
| :--- | :--- |
| `/<tool> [request]` | Request a Runtime Tool, for example `/read README.md`, `/bash npm test`, or `/web_search Pisper`. The Agent still generates arguments and calls the Tool; the active execution mode and Tool settings remain enforced. |
| `/skill:<name> [request]` | Request an enabled Skill, for example `/skill:docs-search find signing requirements`. Runtime resources load on demand, and Tools invoked by the Skill follow the same permission chain. |
