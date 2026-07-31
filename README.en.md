<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper project logo" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>Powered by Pi, whispered by Agents.</strong></p>
<p align="center">Complex work moves quietly in the background; every task receives a clear response.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Tauri-17141F?style=flat-square&logo=tauri&logoColor=F59E0B" alt="Tauri" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest">
    <img src="https://img.shields.io/badge/Download%20Desktop-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="Download Pisper desktop" />
  </a>
</p>

<a id="sponsors"></a>

## ❤️ Sponsors

Thanks to the following partners for supporting the Pisper community. If you'd like to appear here too, feel free to reach out via [Issues](https://github.com/ling-kong-ran/pisper/issues).

<details open>
<summary>View sponsors</summary>

<table>
<tr>
<td width="180" align="center">
  <a href="https://matrix.000328.xyz/sign-up?aff=ZPeH"><strong>Matrix</strong></a>
</td>
<td>
Thanks to <a href="https://matrix.000328.xyz/sign-up?aff=ZPeH">Matrix</a> for supporting the Pisper community. Registering through <a href="https://matrix.000328.xyz/sign-up?aff=ZPeH">this link</a> may generate referral revenue for the project.
</td>
</tr>
</table>

> Sponsor links contain a referral parameter. Sponsor content is not targeted using conversations, workspaces, providers, models, or API configuration, and Pisper does not send that data to sponsors. The public configuration for the in-app sponsor placement is maintained in [`docs/sponsors.json`](./docs/sponsors.json).

</details>

<p align="center">
  <a href="#overview">Overview</a> ·
  <a href="#preview">Interface</a> ·
  <a href="#features">Features</a> ·
  <a href="#desktop-pet">Desktop Pet</a> ·
  <a href="#install">Install</a> ·
  <a href="#tui">Terminal Client</a> ·
  <a href="#development">Development</a> ·
  <a href="#sponsors">Sponsors</a> ·
  <a href="#license">License</a>
</p>

---

<a id="overview"></a>

## Overview

One window per task, context copied back and forth, tool configs scattered everywhere — working with multiple agents shouldn't feel like this.

**Pisper brings every AI Agent into one desktop workspace.** Models, context, tools, and permissions fall into place while complex collaboration happens quietly in the background. You stay focused on the work. Each session runs independently with tabs, splits, and drag-to-dock panels, returning exactly where you left it.

**Product highlights:**

- **Quietly simple, decisively efficient** — Sessions, tools, memory, and execution state come together in one place: fewer windows, fewer configuration surfaces, and less irrelevant context.
- **Capabilities answer on demand** — Progressive disclosure keeps frequent tools lightweight, hot-loads optional schemas when a task needs them, and expands full Skill instructions only for matching work.
- **Parallel voices, uninterrupted flow** — Subagents run non-blocking in isolated contexts while the primary Agent continues replying, accepting new instructions, or delegating independent work.
- **Completed work echoes back** — Subagent output persists and automatically wakes the parent Agent to continue reasoning, with no polling and no result stranded in a passive notification.
- **Clear boundaries, orderly collaboration** — Every session owns its model, context, directory, and permissions, while dynamic loading remains subject to execution modes, workspace boundaries, and approval.

### Get Started in Three Steps

1. [Download the desktop app](https://github.com/ling-kong-ran/pisper/releases/latest) (Windows / macOS / Linux, no Node.js required)
2. Configure any model provider with an API key
3. Create a session and start working in parallel

<a id="preview"></a>

## Interface

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Pisper welcome screen in the dark theme" />
  <br /><sub><strong>Dark workspace</strong> · Every session has its own model, context, directory, and permissions</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat.png" alt="Pisper single-session workspace" />
      <br /><sub><strong>Session workspace</strong> · Focused tasks, tool state, and context usage</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Pisper multi-session dock workspace" />
      <br /><sub><strong>Dock splits</strong> · Tabs, horizontal and vertical splits, and drag-to-dock panels</sub>
    </td>
  </tr>
</table>

### Workspace

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/history.png" alt="Pisper session history" />
      <br /><sub><strong>Session history</strong> · Search, rename, and reopen sessions in any layout</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/assets.png" alt="Pisper asset library" />
      <br /><sub><strong>Assets</strong> · Collect images, files, links, and Agent output</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Pisper channels" />
      <br /><sub><strong>Two-way channels</strong> · Connect Feishu and personal Weixin</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/schedules.png" alt="Pisper schedules" />
      <br /><sub><strong>Schedules</strong> · Configure frequency, execution mode, and notifications</sub>
    </td>
  </tr>
</table>

### Capabilities

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/plugins.png" alt="Pisper tool policy" />
      <br /><sub><strong>Tools</strong> · Manage permissions by risk and execution mode</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Pisper memory view" />
      <br /><sub><strong>Memory</strong> · Search facts and decisions in a persistent knowledge graph</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/mcp.png" alt="Pisper MCP services" />
      <br /><sub><strong>MCP</strong> · Manage services, tool permissions, and call history</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/skills.png" alt="Pisper skills" />
      <br /><sub><strong>Skills</strong> · Install and control reusable capability packages</sub>
    </td>
  </tr>
</table>

### Automation

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflows.png" alt="Pisper workflow list" />
      <br /><sub><strong>Workflows</strong> · Organize automation from presets or drafts</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Pisper workflow builder" />
      <br /><sub><strong>Visual builder</strong> · Connect prompt, file, MCP, and notification nodes</sub>
    </td>
  </tr>
</table>

### Settings

<p align="center">
  <img src="./docs/shots/config.png" alt="Pisper provider and model settings" />
  <br /><sub><strong>Providers and models</strong> · Manage protocols, endpoints, credentials, and model catalogs independently</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/config-notifications.png" alt="Pisper notification settings" />
      <br /><sub><strong>Notifications</strong> · Customize templates for chats, schedules, and workflows</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/config-interface.png" alt="Pisper interface settings" />
      <br /><sub><strong>Interface</strong> · Language, density, and display preferences</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/config-desktop-pet.png" alt="Pisper desktop pet settings" />
      <br /><sub><strong>Desktop pet</strong> · Search, install, and manage pets from Petdex</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/config-updates.png" alt="Pisper app updates" />
      <br /><sub><strong>App updates</strong> · Review version status and update channels</sub>
    </td>
  </tr>
</table>

<a id="features"></a>

## Features

From conversation to automation, Pisper covers the full agent workflow:

| Area | Capability |
| :--- | :--- |
| **Multi-session chat** | Independent Agent sessions, models, and permissions with tab groups, splits, drag-to-dock panels, and layout restoration. |
| **Agent Runtime** | Built on Pi Coding Agent with tool activity, goals, Skills, and asynchronous isolated Subagents. |
| **Progressive disclosure** | Keep frequent tools stable and hot-load optional tool schemas by intent into the current session without restarting it. |
| **Skills** | Implements the Agent Skills standard; only names and descriptions stay in context, while full `SKILL.md` instructions load for matching or explicit invocations. |
| **Asynchronous Subagents** | Run isolated tasks without blocking, persist their results, and automatically wake the parent Agent when work completes. |
| **Tools & MCP** | Manage built-in tools, plugins, and MCP services without exposing credentials to the UI. |
| **Memory** | Store and search preferences, facts, decisions, and tasks in SQLite — it learns you over time. |
| **Multimodal** | Read images, documents, and code, then generate or edit visual content through configured models. |
| **Automation** | Schedules and visual workflows with retries, failure policies, history, and notifications. |
| **Two-way channels** | Connect Feishu and personal Weixin — reach your agents even when you're away from the desk. |
| **Web preview** | Preview external pages in a Dock panel, with a system-browser fallback when needed. |
| **Desktop app** | Tauri 2 system-WebView app with single-instance handling, tray controls, signed updates, and GitHub Releases. |
| **Security boundaries** | Per-session `Read only / Workspace / Full access`, one-shot approval, credential isolation, and local data boundaries. |

### Capabilities Answer on Demand: Progressive Disclosure

Pisper does not flood the context with every capability at once. Tools and knowledge unfold layer by layer as the task develops:

- **A stable hot path** — Frequent file, search, editing, and terminal tools keep the lightweight base prompt stable and cacheable.
- **Hot-loaded tools** — Optional capabilities such as Web Search, browser automation, memory, MCP, and Subagents can be discovered by intent, adding only the most relevant schemas to the current session.
- **Skills on demand** — Only Skill names and descriptions are exposed by default. Full instructions, scripts, and references load when a task matches or the Skill is explicitly invoked.

This reduces fixed tokens in long sessions while keeping irrelevant tools out of model decisions. Dynamic loading remains subject to tool settings, execution modes, workspace boundaries, and approval.

> In the current benchmark, the fixed prompt is about **2,979 tokens** — roughly **58.7% less** than injecting the full catalog. Actual billing and cache savings vary by model and provider.

### Parallel Voices, Unbroken Flow: Non-blocking Subagents

A bounded task can move to an isolated Subagent without stopping the primary Agent in its tracks. Each Subagent inherits the active model, reasoning level, safe tools, and workspace boundary. The parent never needs to poll, and background work never blocks the next reply.

Completed work does not disappear into an unattended notification. If the parent is running, the result joins the active reasoning turn; if the parent is idle, Pisper wakes it through a hidden internal message so every background contribution returns to the main task.

> **Execution boundary:** Workspace mode constrains file access through structured tools. Reads run directly; file modifications and every Shell command require user approval. Shell runs as the current OS user. File modifications and Shell stop requesting per-operation approval only after an explicit switch to full-access mode.

<a id="desktop-pet"></a>

## Desktop Pet

A workspace doesn't have to feel cold. Pisper natively supports [Petdex](https://petdex.dev)-compatible pets — while your agent thinks, the pet paces; when the task finishes, it lets you know. Search, install, switch, enable, or disable pets in **Settings → Desktop pet** — no Petdex CLI or companion process required.

- Waiting, thinking, tool activity, completion, and failure map to pet animations.
- The Tauri desktop app uses an independent transparent window with dragging, always-on-top behavior, multi-display position persistence, and `20%–100%` opacity. It remains visible when the main window is hidden.
- The Web app uses a draggable in-page overlay that exits with the page.
- Assets are downloaded only from HTTPS allowlisted hosts and validated for size, format, sprite grid, and path safety. Community pets are not bundled.

See [`docs/petdex-integration.md`](./docs/petdex-integration.md).

<a id="install"></a>

## Install

### Desktop (Recommended)

Download a Windows, macOS, or Linux installer from [GitHub Releases](https://github.com/ling-kong-ran/pisper/releases/latest). It works out of the box — no separate Node.js install is required.

#### If macOS Refuses to Open Pisper

Pisper is not currently notarized by Apple. Confirm that it came from the official Releases page, then select **Open Anyway** in **System Settings → Privacy & Security**. If that option is unavailable, run:

```bash
sudo xattr -rd com.apple.quarantine /Applications/Pisper.app
```

Use this command only for Pisper downloaded from the official Releases page and placed in `/Applications`.

#### If the Linux AppImage Does Not Start

```bash
chmod +x Pisper_*_linux_x86_64.AppImage
./Pisper_*_linux_x86_64.AppImage
```

If FUSE is missing, install `libfuse2` or `libfuse2t64`, or use the `.deb` package:

```bash
sudo apt install ./Pisper-*-linux-amd64.deb
```

<a id="tui"></a>

### Terminal Client (TUI)

Pisper also provides a Rust + Ratatui terminal client. Every TUI launch opens with the Pisper terminal brand in the main area; once interaction begins, the brand yields to the message stream. When the model provides reasoning tokens, the TUI expands Thinking before the response and marks active reasoning with an event-driven terminal spinner. The TUI reuses the desktop app's Node SEA sidecar, Agent runtime, sessions, Tools, Skills, MCP, execution modes, and approval chain.

After installing the desktop app, install or uninstall the `pisper` command from **Settings → Interface → Pisper terminal command**. Pisper uses a current-user directory and manages the corresponding PATH entry without administrator access; restart the terminal host after making a change. Windows installs `pisper.exe`, while macOS and Linux install the extensionless `pisper` command.

Run it in the current directory from source:

```bash
npm run tui:dev
```

Choose another workspace:

```bash
npm run tui:dev -- --cwd /path/to/project
```

Build a complete directory containing `pisper`, the SEA sidecar, and its runtime:

```bash
npm run sidecar:sea
npm run tui:package
```

Add `release/tui/pisper-<version>-<platform>-<arch>/` to `PATH`, then launch Pisper from any project directory:

```bash
pisper
```

A normal launch always creates an empty conversation and never loads history automatically. Use `resume` explicitly to restore the most recent conversation for the current workspace:

```bash
pisper resume
pisper resume --cwd /path/to/project
```

Diagnose the sidecar, authentication, and capability catalog:

```bash
pisper doctor
```

#### Change Session Permissions

Enter `/mode` to show the current execution mode. Change the current session with:

| Command | Behavior |
| :--- | :--- |
| `/mode read-only` | Exposes only low-risk analysis tools and prevents project modifications. |
| `/mode workspace` | Keeps file tools inside the current workspace; reads run directly, while file modifications and every Shell command require user approval. |
| `/mode full-access` | Allows host file and network access; Shell runs as the current OS user without per-command approval. Use only when explicitly needed. |

#### Invoke a Tool

1. Enter `/` in the composer.
2. Select a Tool marked with `T`.
3. Add the target or arguments after the command, then press `Enter`.

Examples:

```text
/read README.md
/bash npm test
/web_search Pisper latest release
/mcp_pencil_get_editor_state_f9837b9b inspect the current Pencil canvas
```

The TUI sends the selected Tool name to the runtime as a structured request so its schema is active for the turn; the Agent still prepares arguments and performs the call. Slash selection never bypasses Tool settings, the active execution mode, workspace boundaries, or runtime approval. Disabled and unavailable Tools do not appear.

#### Invoke a Skill

Enter `/`, select a Skill marked with `S`, and add the task after its command. If an enabled Skill exposes `/skill:docs-search`, for example:

```text
/skill:docs-search find the signing requirements for the Tauri updater
```

Skill names come from the active runtime, so the Slash list is authoritative. Only enabled, model-invocable Skills appear. The runtime loads the matching `SKILL.md`, scripts, and references on demand. Tool calls made by a Skill remain subject to the current `/mode`, workspace boundary, and approval policy.

#### Built-in TUI Commands

| Command | Action |
| :--- | :--- |
| `/new` | Create a conversation. |
| `/sessions` | Switch conversation history. |
| `/events` | Open the event ledger for the current TUI process. |
| `/chat` | Return to the message stream. |
| `/model` | Show the active model. |
| `/mode` | Show the current execution mode and accepted values. |
| `/quit` | Exit the TUI and stop the sidecar it launched. |

During a run, `Ctrl+C` aborts the active Agent; while idle, it exits. When the runtime requests Tool approval, press `Y` to approve or `N`/`Esc` to deny.

See [`src-tui/README.md`](./src-tui/README.md) for the complete development and release layout.

### Run from Source

Requires Node.js 20+, npm, and at least one model provider with an API key.

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm install
npm run dev
```

The Web development URL defaults to `http://127.0.0.1:5173`. For production mode:

```bash
npm run build
npm start
```

Desktop development and packaging:

```bash
npm run desktop:dev
npm run desktop:pack
```

Configuration, sessions, and memory are stored under `~/.pisper/agent` by default. Set `PISPER_AGENT_DIR` to use another location. When upgrading from Vesper, the legacy `~/.vesper` directory is migrated to `~/.pisper` automatically on first launch (existing data is never overwritten; the legacy directory is kept as a backup).

<a id="development"></a>

## Development

**Stack:** TypeScript, React 19, Vite, Tailwind CSS, shadcn/ui, Dockview, React Flow, Zustand, i18next, Tauri 2, Rust, Node SEA, and Pi Coding Agent.

```bash
npm run check
npm test
npm run build
```

[Issues](https://github.com/ling-kong-ran/pisper/issues) and [pull requests](https://github.com/ling-kong-ran/pisper/pulls) are welcome. Do not commit API keys, bot credentials, or personal data from `~/.pisper/agent`.

<a id="license"></a>

## License

Pisper is released under the [MIT License](./LICENSE). Third-party dependencies and external resources remain under their own licenses and rights notices. Petdex community assets belong to their respective creators or rights holders and are not part of Pisper's source tree or release packages.

## Acknowledgments

Pisper is built on [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) and benefits from open-source projects including Node.js, React, TypeScript, Vite, Tailwind CSS, shadcn/ui, React Router, Zustand, React Flow, and i18next.

Thanks to [Petdex](https://petdex.dev) and [`crafter-station/petdex`](https://github.com/crafter-station/petdex) for the MIT-licensed format and catalog implementation reference.

Contributors:

- [@mik-myp](https://github.com/mik-myp) — Frontend TypeScript architecture, shadcn/ui / AI Elements, Zustand, and i18n refactor ([#1](https://github.com/ling-kong-ran/pisper/pull/1))

<p align="right"><a href="#top">Back to top ↑</a></p>
