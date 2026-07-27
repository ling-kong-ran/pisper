<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper project logo" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>Powered by Pi, whispered by agents.</strong></p>
<p align="center">A multi-agent desktop workspace that brings conversations, tools, memory, and automation into one dockable interface.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Electron-17141F?style=flat-square&logo=electron&logoColor=F59E0B" alt="Electron" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest">
    <img src="https://img.shields.io/badge/Download%20Desktop-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="Download Pisper desktop" />
  </a>
</p>

<p align="center">
  <a href="#overview">Overview</a> ·
  <a href="#preview">Interface</a> ·
  <a href="#features">Features</a> ·
  <a href="#desktop-pet">Desktop Pet</a> ·
  <a href="#install">Install</a> ·
  <a href="#development">Development</a> ·
  <a href="#license">License</a>
</p>

---

<a id="overview"></a>

## Overview

One window per task, context copied back and forth, tool configs scattered everywhere — working with multiple agents shouldn't feel like this.

**Pisper puts all your AI agents into one desktop workspace.** Each session has its own model, context, working directory, and execution permissions. Tabs, splits, and drag-to-dock panels organize them like an IDE — and the layout is right where you left it on restart.

**Why Pisper:**

- **Parallel without chaos** — Run multiple sessions on different tasks at once, fully isolated, with automatic layout restoration.
- **Token-efficient by design** — Dynamic prompts load only the tools the current task needs: a fixed prompt of ~2,979 tokens, roughly 58.7% less than injecting the full catalog.
- **One place for everything** — Memory, schedules, workflows, MCP, and Feishu / Weixin channels under a single roof.
- **Safe by default** — Sensitive actions stay behind execution modes, workspace boundaries, sandboxing, and approval. Nothing escalates without asking you.

### Get Started in Three Steps

1. [Download the desktop app](https://github.com/ling-kong-ran/pisper/releases/latest) (Windows / macOS / Linux, no Node.js required)
2. Configure any model provider with an API key
3. Create a session and start working in parallel

<a id="preview"></a>

## Interface

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Pisper welcome screen in the dark theme" />
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Pisper multi-session dock workspace" />
      <br /><sub><strong>Dock workspace</strong> · Tabs, splits, and drag-to-dock panels</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Pisper memory view" />
      <br /><sub><strong>Memory</strong> · Searchable persistent knowledge</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Pisper workflow builder" />
      <br /><sub><strong>Workflows</strong> · Reusable visual automation</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Pisper channels view" />
      <br /><sub><strong>Two-way channels</strong> · Feishu and personal Weixin</sub>
    </td>
  </tr>
</table>

<a id="features"></a>

## Features

From conversation to automation, Pisper covers the full agent workflow:

| Area | Capability |
| :--- | :--- |
| **Multi-session chat** | Independent Agent sessions, models, and permissions with tab groups, splits, drag-to-dock panels, and layout restoration. |
| **Agent Runtime** | Built on Pi Coding Agent with tool activity, goals, skills, and isolated subagents. |
| **Dynamic prompts** | Keep a stable lightweight prompt, and append cold tool Schemas only from explicit intent. |
| **Tools & MCP** | Manage built-in tools, plugins, and MCP services without exposing credentials to the UI. |
| **Memory** | Store and search preferences, facts, decisions, and tasks in SQLite — it learns you over time. |
| **Multimodal** | Read images, documents, and code, then generate or edit visual content through configured models. |
| **Automation** | Schedules and visual workflows with retries, failure policies, history, and notifications. |
| **Two-way channels** | Connect Feishu and personal Weixin — reach your agents even when you're away from the desk. |
| **Web preview** | Preview external pages in a Dock panel, with a system-browser fallback when needed. |
| **Desktop app** | Electron single-instance app with in-app release notes and GitHub Releases updates. |
| **Security boundaries** | Per-session `Read only / Workspace / Full access`, one-shot approval, secret redaction, and data isolation. |

### Lightweight Prompts: Real Savings on Long Sessions

Ordinary coding sessions load only high-frequency tools. Web Search, browser automation, visual generation, memory, MCP, and subagents are appended only when clearly needed, then return to the lightweight baseline on the next ordinary request. Dynamic activation never bypasses tool settings, execution modes, sandboxing, or approval — saving tokens never means cutting safety.

> In the current benchmark, the fixed prompt is about **2,979 tokens** — roughly **58.7% less** than injecting the full catalog. Actual billing and cache savings vary by model and provider.

> **Sandbox note:** Workspace mode uses [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) to restrict Shell writes, credentials, and network access. First use on Windows requires one UAC prompt. If initialization fails, Pisper blocks execution instead of falling back to full access. The runtime remains a Beta Research Preview.

<a id="desktop-pet"></a>

## Desktop Pet

A workspace doesn't have to feel cold. Pisper natively supports [Petdex](https://petdex.dev)-compatible pets — while your agent thinks, the pet paces; when the task finishes, it lets you know. Search, install, switch, enable, or disable pets in **Settings → Desktop pet** — no Petdex CLI or companion process required.

- Waiting, thinking, tool activity, completion, and failure map to pet animations.
- Electron uses an independent transparent window with dragging, always-on-top behavior, multi-display position persistence, and `20%–100%` opacity. It remains visible when the main window is hidden.
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
chmod +x Pisper-*-linux-x86_64.AppImage
./Pisper-*-linux-x86_64.AppImage
```

If FUSE is missing, install `libfuse2` or `libfuse2t64`, or use the `.deb` package:

```bash
sudo apt install ./Pisper-*-linux-amd64.deb
```

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

**Stack:** TypeScript, React 19, Vite, Tailwind CSS, shadcn/ui, Dockview, React Flow, Zustand, i18next, Node.js, Electron, and Pi Coding Agent.

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
