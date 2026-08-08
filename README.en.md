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
  <a href="https://ling-kong-ran.github.io/pisper/">Project Site</a> ·
  <a href="#features">Features</a> ·
  <a href="#data-safety">Data Safety</a> ·
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

See the [Pisper project site](https://ling-kong-ran.github.io/pisper/) for the product overview and interface preview.

### Get Started in Three Steps

1. [Download the desktop app](https://github.com/ling-kong-ran/pisper/releases/latest) (Windows / macOS / Linux, no Node.js required)
2. Configure any model provider with an API key
3. Create a session and start working in parallel

<a id="features"></a>

## Features

- **Multiple sessions:** independent models, context, workspaces, and permissions with Dock splits and layout restoration.
- **Tools, Skills, and MCP:** manage capabilities and call permissions in one place.
- **Subagents:** run temporary tasks in isolated contexts and return results to the parent session.
- **Memory and multimodal input:** retrieve project memory and process images, documents, and code.
- **Automation and channels:** schedules, visual workflows, Feishu, and personal Weixin.
- **Desktop and terminal:** Tauri desktop app and Ratatui TUI.
- **Permission controls:** `Read only / Full access` execution modes, one-shot approval, and credential isolation.

<a id="data-safety"></a>

## Data Safety & Privacy

Pisper is local-first and does not provide a cloud service that hosts or relays conversations. Sessions and settings are stored under `~/.pisper/agent` by default; set `PISPER_AGENT_DIR` to move them.

- The Runtime listens on `127.0.0.1` by default, and Pi telemetry is disabled by default.
- Model calls and enabled remote services such as MCP, Web search, and messaging receive the data required for each request.
- Common credential formats are redacted at storage and display boundaries, but this is not a replacement for DLP or end-to-end encryption. Protect the local Agent data directory and its backups.

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

### Component Updates

**Settings → Updates** in the desktop app is the unified update entry point. A single **Check for updates** action checks the Desktop package, TUI client, and Runtime together. Install controls appear only for components with a newer version, so a small component update does not require downloading the full desktop installer. Each component uses an independent signed Release channel. Runtime updates take effect after restarting the app, and Pisper falls back to the versions bundled with the desktop package if an installed component is unavailable or fails to start.

<a id="tui"></a>

### Terminal Client (TUI)

With Node.js 20+, you can install the Pisper CLI through npm. Installation obtains and verifies the TUI and Runtime for the current platform; it does not install the desktop shell or Web frontend:

```bash
npm install -g pisper
pisper
```

After entering the TUI for the first time, use `/apikey` to choose a Provider and save its API key in a masked input. For visual configuration, run `pisper web`; Pisper installs the signed Web frontend on demand and opens an authenticated, localhost-only settings page in your default browser. **Save Provider settings** does not change the default model; only **Set as default Provider** changes the default for later sessions.

After installing the desktop app, you can also install the `pisper` command from **Settings → Terminal**. The first installation remains explicit; after later desktop updates restart Pisper, it automatically refreshes this managed terminal client:

```bash
pisper                 # start a new conversation
pisper resume          # resume from a list across all workspaces
pisper doctor          # diagnose the runtime
pisper web             # install and open the optional Web settings
pisper update --check  # check TUI, Runtime, and optional Web updates
pisper --help          # show complete onboarding and command help
```

`pisper update all` updates only the TUI and Runtime. Web remains opt-in and can be updated separately with `pisper update web`. See the **[Pisper TUI user guide](./src-tui/README.en.md)** for installation, commands, keyboard controls, attachments, execution modes, and approvals.

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
