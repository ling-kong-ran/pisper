<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/banner.en.svg" width="880" alt="Pisper — Ideas don't queue" />
</p>

<h3 align="center">Run several agents at once — see who needs you, who's running and who's done.</h3>

<p align="center">
  A multi-agent app for desktop, terminal and mobile. All three share one core, so you can switch devices and pick up right where you left off.
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest"><img src="https://img.shields.io/github/v/release/ling-kong-ran/pisper?style=flat-square&label=Desktop&color=F59E0B" alt="Latest desktop release" /></a>
  <a href="https://www.npmjs.com/package/pisper"><img src="https://img.shields.io/npm/v/pisper?style=flat-square&label=npm&color=17141F" alt="npm" /></a>
  <a href="https://github.com/ling-kong-ran/pisper/stargazers"><img src="https://img.shields.io/github/stars/ling-kong-ran/pisper?style=flat-square&label=Stars&color=17141F" alt="GitHub Stars" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/Windows%20·%20macOS%20·%20Linux%20·%20Android%20·%20iOS-17141F?style=flat-square" alt="Platforms" />
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest"><img src="https://img.shields.io/badge/Download_Desktop-Terminal%20UI%20%2B%20Runtime%20included-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="Download Pisper Desktop" /></a>
</p>

<p align="center">
  <a href="https://pisper.cc">Website</a> ·
  <a href="https://pisper.cc/guide.html">Guide</a> ·
  <a href="https://pisper.cc/changelog.html">Changelog</a> ·
  <a href="#quickstart">Install</a> ·
  <a href="#features">Features</a> ·
  <a href="#data-safety">Data safety</a>
</p>

<p align="center">
  <a href="docs/shots/desktop-overview.png"><img src="docs/shots/web/desktop-overview.webp" width="880" alt="Pisper desktop, All agents: grouped into Needs you, Running and Done today; pending commands can be allowed or denied in place" /></a>
</p>

<a id="why"></a>

## ✨ Why Pisper

<table>
  <tr>
    <td width="33%" valign="top">
      <h4>⚡ Many things at once</h4>
      One row per agent, grouped into Needs you, Running and Done today. You see at a glance who is waiting for approval, and allow or deny risky commands right there — no clicking into each session.
    </td>
    <td width="33%" valign="top">
      <h4>🌿 Take a wrong turn, go back</h4>
      The session tree keeps every branch: rewind files turn by turn or branch off from any turn while the original conversation stays untouched. Mark key turns and jump back any time.
    </td>
    <td width="33%" valign="top">
      <h4>📱 Switch devices, keep the context</h4>
      Desktop, terminal and phone share one core. Agents on the phone run on the phone itself, and it can pair with your desktop so you can approve commands on the go.
    </td>
  </tr>
</table>

<table>
  <tr>
    <td width="70%" valign="top">
      <a href="docs/shots/desktop-session.png"><img src="docs/shots/web/desktop-session.webp" alt="Desktop session: the agent's actions fold into one line and the reply lists the changed files" /></a>
      <p align="center"><sub>Desktop · session</sub></p>
      <a href="docs/shots/cli.png"><img src="docs/shots/web/cli.webp" alt="Ratatui terminal UI" /></a>
      <p align="center"><sub>Terminal · Ratatui</sub></p>
    </td>
    <td width="30%" valign="top">
      <img src="docs/shots/web/mobile-session.webp" alt="Phone session: the agent runs on the phone itself" />
      <p align="center"><sub>Phone · on-device</sub></p>
    </td>
  </tr>
</table>

<a id="features"></a>

## 🗺️ Features

| | Feature | What it does |
| :---: | --- | --- |
| 🧵 | **Parallel agents** | The sidebar keeps Needs you, Running and recent sessions in view. Each session has its own model, folder and permissions; approvals and questions are handled as cards in the conversation. `Ctrl/⌘ + K` opens the command palette to jump across sessions. |
| 🌳 | **Session tree & branches** | Branch off from any turn, rewind files turn by turn, mark turns; switching paths never deletes context. |
| 🧭 | **Plan · Goal · Team** | Three ways to drive work: plan first, keep going until a goal is met, or hand it to a small team of agents. |
| 🪄 | **Sub-agents** | Pick a model for each of the fast / standard / strong tiers: cheap fast models for searching and sorting, strong ones for design and debugging, with tools narrowed per task. |
| 🛡️ | **Approvals & permission rules** | Three levels — Review before writing, Auto-approve, Full access. Allow or deny commands in bulk by prefix; dangerous commands are stopped separately. |
| 🌐 | **Web & computer use** | Agents open and operate websites in the side browser, keeping your sign-ins; Computer Use operates desktop apps. Clicks, form input and computer actions ask for approval first. |
| 🖥️ | **Remote machines** | Connect another machine over SSH or an executor node; file and command tools run there and approvals still apply. |
| 🧩 | **Tools · plugins · MCP** | Core tools stay loaded while plugins, MCP servers and skills are activated on demand and retired when done. Missing a capability? Ask, and the agent writes a plugin. You can also turn on the built-in MCP server so other local agents can use Pisper. |
| 🎙️ | **Voice input & conversation mode** | Speak, edit the transcript, send; in conversation mode it submits automatically and replies by voice. |
| 🔀 | **Visual workflows** | Wire prompts, files, MCP and notifications into a graph and let the flow handle repetitive work. |
| ⏰ | **Schedules & notifications** | Tasks start on time and results reach you through Feishu, WeChat and other channels. |
| 🧠 | **Memory** | Preferences, decisions and facts are remembered directly and outdated ones retire on their own; drag the timeline to see how they changed. |
| 🔁 | **Models & fallbacks** | Configure multiple providers; when the primary model is still unavailable after retries or out of quota, Pisper switches to backup models in order and keeps going. |
| 🎨 | **Native components** | Describe a component in one sentence and an agent builds it; place it in the component bar, the session inspector, a full page or a floating window. Imports show source and scan results before you trust them, and every change can be rolled back. |
| 📦 | **Git · SVN · desktop pets** | See workspace changes for Git and SVN in the inspector — plus desktop pets from Petdex to keep you company. |

> See the **[Guide](https://pisper.cc/guide.html)** for how to use each one.

<a id="bots"></a>

## 🤖 Pisper Bot <sup>Labs</sup>

<a href="docs/shots/desktop-bot.png"><img src="docs/shots/web/desktop-bot.webp" width="420" align="right" alt="A long-running conversation with a Bot: after fixing a blank page in Safari it deploys a preview" /></a>

Make a few Bots for the work you do often. Each Bot is a conversation that stays open, plus a profile of who it is and what it looks after. Open it and keep talking — it remembers what came before. It edits files and runs commands on its own, and only comes to you for risky actions or things it can't do.

- **Call it from chat apps**: send a line in Feishu or WeChat and it gets to work.
- **Put it on a schedule**: daily dependency checks or reports start on time.
- **Send it to another machine**: dispatch work to a remote computer.
- **Put several Bots in a group**: @ someone to wake them; without an @, the room picks who should answer and wakes only them.

Bots are off by default — turn them on in **Settings › Labs**. Turning them off hides Bots and their sessions but keeps the records. See the [Bot guide](https://pisper.cc/guide.html#bots).

<br clear="right" />

<a id="quickstart"></a>

## 🚀 Install

### 💻 Desktop (recommended)

Download the installer for your platform from [Releases](https://github.com/ling-kong-ran/pisper/releases/latest). **The terminal UI and Runtime are included — no Node.js needed.**

| System | Installer | Notes |
| --- | --- | --- |
| **Windows x64** | `*-setup.exe` | Standard installer (recommended). Reuses the system WebView2 and installs it online if missing. Auto-update uses this one. |
| | `*-offline-setup.exe` | Full offline installer with the WebView2 installer bundled, for offline machines without WebView2. Noticeably larger. |
| **macOS** | `*.dmg` | Apple Silicon only, uses the system WKWebView. |
| **Linux x64** | `*.AppImage` / `*.deb` | AppImage may need FUSE; the DEB may depend on WebKitGTK and other system libraries. |

The offline installer does not include on-demand local voice models, and chatting needs a reachable model service (an intranet one is fine).

<details>
<summary><b>macOS says the app can't be opened?</b></summary>
<br />

Pisper is not notarized by Apple yet. After confirming the download came from the official Releases page, choose **Open Anyway** in **System Settings → Privacy & Security**. If the option is missing:

```bash
sudo xattr -rd com.apple.quarantine /Applications/Pisper.app
```

Only use this for an app downloaded from the official Releases page and placed in `/Applications`.

</details>

<details>
<summary><b>Linux AppImage won't start?</b></summary>
<br />

```bash
chmod +x Pisper_*_linux_x86_64.AppImage
./Pisper_*_linux_x86_64.AppImage
```

If FUSE is missing, install `libfuse2` or `libfuse2t64`, or use the DEB instead:

```bash
sudo apt install ./Pisper_*_linux_x86_64.deb
```

If it fails with a system library error before starting, launch it from a terminal to see which library is missing.

</details>

### 📱 Mobile app

Installers are on [App Releases](https://github.com/ling-kong-ran/pisper/releases?q=app-v). On first launch, the Android / iOS app starts its bundled on-device Runtime — agents, tools and sessions run on the phone; it is not a remote control for your desktop.

| Platform | Installer | Notes |
| --- | --- | --- |
| **Android** | Signed arm64 APK | Installs directly; allow the install source when sideloading for the first time. |
| **iOS** | Unsigned IPA | Cannot be installed as is; re-sign it with AltStore, Sideloadly or your own developer account. |

<details>
<summary><b>Connect to your desktop</b></summary>
<br />

1. For the first pairing, put the phone and computer on the same LAN, then turn on **Settings → Remote access** on the desktop.
2. On the phone, open **Settings → Servers → Add server**, allow local network access, pick the discovered desktop and send a request. A device token is issued only after you approve it on the desktop.
3. When you are not on the same LAN, generate a one-time QR code on the desktop and scan it or type it in. Connections prefer the LAN and fall back to Iroh P2P; every path checks the TLS fingerprint and uses a device token.

The phone remembers whether you use the local or a remote Runtime, and hides features the phone can't run, such as Shell, MCP and workflows.

</details>

### ⌨️ npm

Requires Node.js 20 or later; installing downloads the terminal UI and Runtime.

```bash
npm i -g pisper
pisper            # terminal UI; run /provider first to set up a model
pisper web        # open the web UI
```

See the [TUI command reference](./src-tui/README.md) for commands and options.

### 🧪 From source

<details>
<summary><b>Expand</b></summary>
<br />

Requires Node.js 24, npm, and at least one model provider with an API key.

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm ci
npm run dev                   # web and API, http://127.0.0.1:5173 by default
npm run desktop:webview:dev   # desktop shell
```

</details>

Data lives in `~/.pisper/agent` by default; set `PISPER_AGENT_DIR` to move it.

<a id="data-safety"></a>

## 🔒 Data safety

Pisper has no "our cloud". Sessions, memory and settings stay on your own devices; only the model services, external MCP servers, search and channels you configure and actually use receive the data their task needs.

- **Local only by default**: the desktop Runtime binds to 127.0.0.1 and the on-device Runtime on a phone listens on a random loopback port. LAN HTTPS and Iroh P2P start only after you turn on remote access, protected by TLS fingerprints and device tokens. Pi telemetry is off by default.
- **Secrets redacted first**: common API keys, Bearer/JWT tokens, private keys and connection strings are replaced before memory is written or summaries are shown. Credentials are never echoed to agents through normal interfaces, and the host shell strips common credential environment variables.
- **Bounded permissions**: Review before writing, Auto-approve, or Full access; dangerous commands and browser or computer actions are confirmed separately.
- **External MCP is opt-in**: the built-in MCP server is off by default, listens only on 127.0.0.1 and uses its own token.
- **Traceable memory**: every change is logged and can be undone. What you said yourself is never overridden by model inferences, and deleting something removes it together with the original conversation text.

> **Limits**: redaction only recognizes common formats; it is not full DLP, sandboxing or end-to-end encryption. Provider credentials live in the local data folder (on phones, in the app's private folder — not yet in Keystore / Keychain), so protect that folder and its backups.

## 🔄 Independent updates

Desktop, TUI, Runtime and the mobile app each have their own versions, signatures and updates, and fall back to the bundled version if an update fails. On desktop, component updates are checked from Settings in one place.

<a id="pi-runtime"></a>

## 🧠 Built on Pi Coding Agent

Pisper uses [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) as its agent runtime and builds a product layer for everyday multi-agent work on top of Pi's model access and tool execution:

- **Session orchestration**: parallel sessions, the session tree, working folders and permission policies, sub-agents and Bots, organized into a multi-agent system that runs for the long haul.
- **Context & caching**: hot/cold tool tiers, discover/call on-demand loading, stably ordered tool definitions and prompt-shape diagnostics keep provider prompt caches hitting.
- **Three surfaces**: Tauri desktop, Ratatui terminal and Android / iOS; the phone's on-device mode runs the same Node Runtime, HTTP/SSE and React UI.

<a id="development"></a>

## 🛠️ Development

The product layer is built mainly with React, TypeScript, Tauri, Rust and Node SEA.

```bash
npm run check   # types, lint, i18n, formatting and startup checks
npm test        # every test under runtime/tests
npm run build   # production build (with size budget and Safari compatibility checks)
```

Repository conventions are in [AGENTS.md](./AGENTS.md). [Issues](https://github.com/ling-kong-ran/pisper/issues) and [pull requests](https://github.com/ling-kong-ran/pisper/pulls) are welcome; please don't commit API keys, bot credentials or personal data from `~/.pisper/agent`.

## 📚 Docs

| Doc | Contents |
| --- | --- |
| [Guide](https://pisper.cc/guide.html) | Everything from installing and setting up models to Bots, workflows, memory and multiple devices |
| [Changelog](https://pisper.cc/changelog.html) | What changed in each version |
| [TUI command reference](./src-tui/README.md) | Terminal UI commands, slash commands and options |
| [Privacy](https://pisper.cc/privacy.html) · [Support](https://pisper.cc/support.html) | How data is handled and where to get help |

## 🙏 Acknowledgements

Thanks to [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent), [Petdex](https://petdex.dev) and the open-source software this project uses.

Contributors:

- [@mik-myp](https://github.com/mik-myp) — front-end TypeScript architecture, shadcn/ui / AI Elements, Zustand and i18n refactor ([#1](https://github.com/ling-kong-ran/pisper/pull/1))

---

<p align="center">
  <strong>If Pisper helps you, a ⭐ is the biggest boost for keeping it growing.</strong><br />
  <sub>And feel free to share it with anyone who uses agents to get real work done.</sub>
</p>

<p align="center">
  <a href="./LICENSE">MIT License</a> · © Pisper Contributors ·
  <a href="#top">Back to top ↑</a>
</p>
