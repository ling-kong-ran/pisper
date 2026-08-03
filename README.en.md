<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper project logo" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>A multi-Agent workspace powered by Pi</strong></p>

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

Pisper is a desktop and terminal client built on [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent), with multiple sessions, Tools, Skills, MCP, automation, and per-session permissions.

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

<p align="center">
  <a href="./src-tui/README.en.md"><img src="./docs/shots/cli.png" alt="Pisper TUI" /></a>
  <br /><sub><strong>Terminal client</strong></sub>
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

- **Multiple sessions:** independent models, context, workspaces, and permissions with Dock splits and layout restoration.
- **Tools, Skills, and MCP:** manage capabilities and call permissions in one place.
- **Subagents:** run temporary tasks in isolated contexts and return results to the parent session.
- **Memory and multimodal input:** retrieve project memory and process images, documents, and code.
- **Automation and channels:** schedules, visual workflows, Feishu, and personal Weixin.
- **Desktop and terminal:** Tauri desktop app and Ratatui TUI.
- **Permission controls:** `Read only / Workspace / Full access`, one-shot approval, and credential isolation.

<a id="desktop-pet"></a>

## Desktop Pet

Pisper supports [Petdex](https://petdex.dev)-compatible pets. Install and manage them under **Settings → Desktop pet**. See [`docs/petdex-integration.md`](./docs/petdex-integration.md).

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

After installing the desktop app, install the `pisper` command from **Settings → Terminal**. The first installation remains explicit; after later desktop updates restart Pisper, it automatically refreshes this managed terminal client:

```bash
pisper          # start a new conversation
pisper resume   # resume from an interactive list across all workspaces
pisper doctor   # diagnose the runtime
```

See the **[Pisper TUI user guide](./src-tui/README.en.md)** for installation, commands, keyboard controls, attachments, execution modes, and approvals.

### Run from Source

Requires Node.js 20+, npm, and at least one model provider with an API key.

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm install
npm run dev
```

Desktop development and packaging:

```bash
npm run desktop:webview:dev
npm run desktop:webview:build
```

Data is stored under `~/.pisper/agent` by default. Set `PISPER_AGENT_DIR` to use another location.

<a id="development"></a>

## Development

Main stack: React, TypeScript, Tauri, Rust, Node SEA, and Pi Coding Agent.

```bash
npm run check
npm test
npm run build
```

[Issues](https://github.com/ling-kong-ran/pisper/issues) and [pull requests](https://github.com/ling-kong-ran/pisper/pulls) are welcome. Do not commit API keys, bot credentials, or personal data from `~/.pisper/agent`.

<a id="license"></a>

## License

Pisper is released under the [MIT License](./LICENSE). Third-party dependencies and community resources retain their own licenses.

## Acknowledgments

Thanks to [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent), [Petdex](https://petdex.dev), and the open-source software used by this project.

Contributors:

- [@mik-myp](https://github.com/mik-myp) — Frontend TypeScript architecture, shadcn/ui / AI Elements, Zustand, and i18n refactor ([#1](https://github.com/ling-kong-ran/pisper/pull/1))

<p align="right"><a href="#top">Back to top ↑</a></p>
