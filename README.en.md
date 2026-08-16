<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper project logo" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>A local-first Agent application powered by Pi</strong></p>

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

The product overview, interface preview, and onboarding entry point are maintained on the [Pisper project site](https://ling-kong-ran.github.io/pisper/).

<a id="features"></a>

## Features

See the [product and capabilities sections](https://ling-kong-ran.github.io/pisper/#product) for the complete feature overview and interface demos. Plugin details live in the [local plugin guide](./docs/local-plugins.md) and [plugin authoring guide](./docs/plugin-authoring.en.md).

<a id="data-safety"></a>

## Data Safety & Privacy

Local data boundaries, third-party data flows, credential redaction coverage, and limitations are documented in the [project site's data safety section](https://ling-kong-ran.github.io/pisper/#safety).

<a id="desktop-pet"></a>

## Desktop Pet

Pisper supports [Petdex](https://petdex.dev)-compatible pets. Install and manage them under **Settings → Desktop pet**. See [`docs/petdex-integration.md`](./docs/petdex-integration.md).

<a id="install"></a>

## Install

### Desktop (Recommended)

See the [Pisper project site](https://ling-kong-ran.github.io/pisper/) for desktop downloads, supported platforms, and basic installation guidance.

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

See the [project site](https://ling-kong-ran.github.io/pisper/#updates) for the independent Desktop, TUI, and Runtime update model.

<a id="tui"></a>

### Terminal Client (TUI)

See the [project site's terminal section](https://ling-kong-ran.github.io/pisper/#terminal) for the installation entry point. The **[Pisper TUI user guide](./src-tui/README.en.md)** covers installation, updates, commands, keyboard controls, attachments, Provider setup, execution modes, and approvals.

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
