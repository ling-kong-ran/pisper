<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/vesper-logo.svg" width="112" alt="Vesper project logo" />
</p>

<h1 align="center">Vesper</h1>

<p align="center"><strong>When daylight fades, ideas stay awake.</strong></p>
<p align="center">A local-first multi-agent workspace for conversations, tools, memory, and automation in one dockable interface.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Electron-17141F?style=flat-square&logo=electron&logoColor=F59E0B" alt="Electron" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/vesper/releases/latest">
    <img src="https://img.shields.io/badge/Download%20Desktop-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="Download Vesper desktop" />
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

Vesper is a **local-first multi-agent workspace**. Each session has its own model, context, working directory, and execution permissions. Tabs, splits, and drag-to-dock panels turn them into an IDE-style workspace.

- Run independent sessions in parallel and restore their layout automatically.
- Load only the tools required by the current task through dynamic prompts.
- Manage memory, schedules, workflows, MCP, and external channels in one app.
- Keep sensitive actions behind execution modes, workspace boundaries, sandboxing, and approval.

<a id="preview"></a>

## Interface

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Vesper welcome screen in the dark theme" />
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Vesper multi-session dock workspace" />
      <br /><sub><strong>Dock workspace</strong> · Tabs, splits, and drag-to-dock panels</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Vesper memory view" />
      <br /><sub><strong>Memory</strong> · Searchable local knowledge</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Vesper workflow builder" />
      <br /><sub><strong>Workflows</strong> · Reusable visual automation</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Vesper channels view" />
      <br /><sub><strong>Two-way channels</strong> · Feishu and personal Weixin</sub>
    </td>
  </tr>
</table>

<a id="features"></a>

## Features

| Area | Capability |
| :--- | :--- |
| **Multi-session chat** | Independent Agent sessions, models, and permissions with tab groups, splits, drag-to-dock panels, and layout restoration. |
| **Agent Runtime** | Built on Pi Coding Agent with tool activity, goals, skills, and isolated subagents. |
| **Dynamic prompts** | Keep a stable lightweight prompt and append cold tool Schemas only from explicit intent. |
| **Tools & MCP** | Manage built-in tools, plugins, and MCP services without exposing credentials to the client. |
| **Memory** | Store and search preferences, facts, decisions, and tasks in local SQLite. |
| **Multimodal** | Read images, documents, and code, then generate or edit visual content through configured models. |
| **Automation** | Schedules and visual workflows with retries, failure policies, history, and notifications. |
| **Two-way channels** | Connect Feishu and personal Weixin with separate models, workspaces, and attachments. |
| **Web preview** | Preview external pages in a Dock panel with a system-browser fallback. |
| **Desktop app** | Electron single-instance app with in-app release notes and GitHub Releases updates. |
| **Security boundaries** | Per-session `Read only / Workspace / Full access`, one-shot approval, secret redaction, and isolated local data. |

### Lightweight Prompts

Ordinary coding sessions load only high-frequency tools. Web Search, browser automation, visual generation, memory, MCP, and subagents are appended only when clearly needed, then return to the lightweight baseline on the next ordinary request. Dynamic activation never bypasses tool settings, execution modes, sandboxing, or approval.

> In the current benchmark, the fixed prompt is about **2,979 tokens**, roughly **58.7% less** than injecting the full catalog. Actual billing and cache savings vary by model and provider.

> **Sandbox note:** Workspace mode uses [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) to restrict Shell writes, credentials, and network access. First use on Windows requires one UAC prompt. Vesper blocks execution if initialization fails instead of falling back to full access. The runtime remains a Beta Research Preview.

<a id="desktop-pet"></a>

## Desktop Pet

Vesper natively supports [Petdex](https://petdex.dev)-compatible pets. Search, install, switch, enable, or disable them in **Settings → Desktop pet** without Petdex CLI or a companion process.

- Waiting, thinking, tool activity, completion, and failure map to pet animations.
- Electron uses an independent transparent window with dragging, always-on-top behavior, multi-display position persistence, and `20%–100%` opacity. It remains visible when the main window is hidden.
- The Web app uses a draggable in-page overlay that exits with the page.
- Assets are downloaded only from HTTPS allowlisted hosts and validated for size, format, sprite grid, and path safety. Community pets are not bundled.

See [`docs/petdex-integration.md`](./docs/petdex-integration.md).

<a id="install"></a>

## Install

### Desktop

Download a Windows, macOS, or Linux installer from [GitHub Releases](https://github.com/ling-kong-ran/vesper/releases/latest). Node.js is not required.

#### If macOS Refuses to Open Vesper

Vesper is not currently notarized by Apple. Confirm that it came from the official Releases page and first select **Open Anyway** in **System Settings → Privacy & Security**. If that option is unavailable, run:

```bash
sudo xattr -rd com.apple.quarantine /Applications/Vesper.app
```

Use this command only for Vesper downloaded from the official Releases page and installed in `/Applications`.

#### If the Linux AppImage Does Not Start

```bash
chmod +x Vesper-*-linux-x86_64.AppImage
./Vesper-*-linux-x86_64.AppImage
```

If FUSE is missing, install `libfuse2` or `libfuse2t64`, or use the `.deb` package:

```bash
sudo apt install ./Vesper-*-linux-amd64.deb
```

### Run from Source

Requires Node.js 20+, npm, and at least one model provider and API key.

```bash
git clone https://github.com/ling-kong-ran/vesper.git
cd vesper
npm install
npm run dev
```

The Web development URL is `http://127.0.0.1:5173` by default. Production mode:

```bash
npm run build
npm start
```

Desktop development and local packaging:

```bash
npm run desktop:dev
npm run desktop:pack
```

Configuration, sessions, and memory are stored in `~/.vesper/agent` by default. Set `VESPER_AGENT_DIR` to use another location.

<a id="development"></a>

## Development

**Stack:** TypeScript, React 19, Vite, Tailwind CSS, shadcn/ui, Dockview, React Flow, Zustand, i18next, Node.js, Electron, and Pi Coding Agent.

```bash
npm run check
npm test
npm run build
```

[Issues](https://github.com/ling-kong-ran/vesper/issues) and [pull requests](https://github.com/ling-kong-ran/vesper/pulls) are welcome. Do not commit API keys, bot credentials, or local data from `~/.vesper/agent`.

<a id="license"></a>

## License

Vesper is open source under the [MIT License](./LICENSE). Third-party dependencies and external resources remain under their own licenses and rights notices. Petdex community assets belong to their respective creators or rights holders and are not part of Vesper's source or release packages.

## Acknowledgments

Vesper is built on [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) and benefits from open-source projects including Node.js, React, TypeScript, Vite, Tailwind CSS, shadcn/ui, React Router, Zustand, React Flow, and i18next.

Thanks to [Petdex](https://petdex.dev) and [`crafter-station/petdex`](https://github.com/crafter-station/petdex) for the MIT-licensed format and catalog implementation reference.

Contributors:

- [@mik-myp](https://github.com/mik-myp) — Frontend TypeScript architecture, shadcn/ui / AI Elements, Zustand, and i18n refactor ([#1](https://github.com/ling-kong-ran/vesper/pull/1))

<p align="right"><a href="#top">Back to top ↑</a></p>
