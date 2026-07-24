<p align="right"><a href="./README.md">简体中文</a> · <strong>English</strong></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/vesper-logo.svg" width="112" alt="Vesper project logo" />
</p>

<h1 align="center">Vesper</h1>

<p align="center"><strong>When daylight fades, ideas stay awake.</strong></p>
<p align="center">A local-first multi-agent workspace where conversations, tools, memory, and workflows move together in one constellation.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Electron-17141F?style=flat-square&logo=electron&logoColor=F59E0B" alt="Electron" />
  <img src="https://img.shields.io/badge/Vite-17141F?style=flat-square&logo=vite&logoColor=F59E0B" alt="Vite" />
  <img src="https://img.shields.io/badge/i18next-17141F?style=flat-square&logo=i18next&logoColor=F59E0B" alt="i18next" />
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/vesper/releases/latest">
    <img src="https://img.shields.io/badge/Download%20Desktop-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="Download Vesper desktop installers" />
  </a>
</p>
<p align="center"><sub>Install Vesper directly—no source checkout or Node.js development environment required.</sub></p>

<p align="center">
  <a href="#about">The Evening Star</a> ·
  <a href="#glance">Vesper at a Glance</a> ·
  <a href="#lightweight-prompt">Lightweight Prompt</a> ·
  <a href="#capabilities">Constellation of Capabilities</a> ·
  <a href="#architecture">Code Map</a> ·
  <a href="#start">Begin Here</a> ·
  <a href="#contributing">Contributing</a>
</p>

---

<a id="about"></a>

## ✦ The Evening Star

**Vesper** takes its name from evening—and from the first star to appear at dusk.

When the noise of daylight recedes, a small light remains beside unfinished work. Vesper is built to be that kind of presence: quiet, lucid, and always within reach, gathering scattered models, tools, and context into a workspace you can see, control, and continue to shape.

Vesper is a **local-first multi-agent workspace**. Run independent sessions in parallel, each with its own model, execution mode, context, and working directory. Arrange them in an IDE-style workspace with draggable tabs, `Split Left` / `Split Right`, adjustable panes, and layouts that restore automatically.

> Let every agent follow its own orbit, and every thought find a place to return to.

- **Parallel without disorder** — Sessions stay independent, with clear state, context, and permissions.
- **Lightweight without compromise** — Dynamic prompts carry only the tools needed for the current task, keeping ordinary coding sessions exceptionally lean.
- **Automation without overreach** — Schedules and workflows carry repetitive work; sensitive actions remain yours to approve.
- **Memory without noise** — Preferences, facts, decisions, and file relationships settle into local, durable memory.
- **Connection without surrender** — MCP, plugins, and channels expand the workspace while staying inside visible permission boundaries.

---

<a id="glance"></a>

## ✦ Vesper at a Glance

![Vesper product demo](./docs/show.gif)

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Vesper multi-session dock workspace" />
      <br />
      <sub><strong>Dock workspace</strong> · Run tasks in parallel with tabs, splits, and drag-to-dock panels</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Vesper memory view" />
      <br />
      <sub><strong>Memory</strong> · Keep meaningful thoughts and decisions glowing within reach</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Vesper workflow builder" />
      <br />
      <sub><strong>Workflows</strong> · Turn an idea into a reusable path that can run</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Vesper channels view" />
      <br />
      <sub><strong>Two-way channels</strong> · Bring Vesper into Feishu and personal Weixin</sub>
    </td>
  </tr>
</table>

---

<a id="lightweight-prompt"></a>

## ✦ Dynamic Prompts · Exceptionally Lightweight

Vesper does not inject every tool, MCP integration, visual capability, browser rule, memory instruction, and multi-agent API into every model call. The runtime keeps a compact, stable base prompt and appends only the tool Schemas explicitly required by the user's current request.

<table>
  <tr>
    <td width="33%" valign="top">
      <strong>Lean by default</strong><br />
      Ordinary coding sessions load only high-frequency file, search, Shell, and task-progress tools. In the current benchmark, fixed input is about <strong>2,979 tokens</strong>—approximately <strong>58.7% less</strong> than injecting the complete tool catalog.
    </td>
    <td width="33%" valign="top">
      <strong>Capabilities on demand</strong><br />
      Web Search, browser automation, visual generation, memory, MCP, and Subagent tools are appended only when explicitly requested. A new ordinary request returns to the lean baseline automatically.
    </td>
    <td width="33%" valign="top">
      <strong>Prompt-cache friendly</strong><br />
      The system prompt and high-frequency tool Schemas remain a stable prefix. Cold Schemas are appended at the end, maximizing opportunities to reuse Provider prompt caches and reduce repeated input and latency.
    </td>
  </tr>
</table>

- **Explicit-only Skills** — Large specialist workflows can be exposed only through `/skill:name`; their names, descriptions, and bodies add zero persistent model-context overhead.
- **Trusted intent routing** — Capabilities activate only from the latest user request and explicit attachment metadata. Text found in web pages, files, tool output, memory, or Agent results cannot silently expand the tool set.
- **Negation-aware activation** — Requests such as “do not use the browser” or “Do not use MCP” do not activate those tools by mistake.
- **Permissions remain hard boundaries** — Dynamic activation decides what is useful for the current turn; it never bypasses tool settings, execution modes, workspace boundaries, sandboxing, or approval policies.

> Token figures use Vesper's conservative `ceil(characters / 4)` estimate and a reproducible benchmark of the current default full mode. Actual Provider billing, cache hits, and latency savings vary by model and service.

---

<a id="capabilities"></a>

## ✦ Constellation of Capabilities

| Area | Capability |
| :--- | :--- |
| **Multi-session chat** | Run independent Agent sessions in parallel with distinct models, execution modes, context, and working directories, plus tab groups, left/right splits, drag-to-dock panels, resizing, and layout restoration. |
| **Agent Runtime** | Built on Pi Coding Agent with structured tool activity, goals, reusable skills, and isolated subagent delegation. |
| **Dynamic prompts** | Keep a stable, lightweight, cache-friendly base prompt and append cold tool Schemas only from explicit user intent, with zero-persistent-overhead explicit Skills. |
| **Tools & MCP** | Bring built-in tools, application plugins, and MCP services into one capability layer, with credential-safe structured configuration. |
| **Memory** | Store preferences, facts, decisions, and tasks in lightweight local SQLite memory, scoped by workspace and searchable, editable, or captured from conversations. |
| **Multimodal** | Read images, documents, and code, then generate or edit visual content through configured OpenAI-compatible, Gemini, Imagen, Veo, or xAI models. |
| **Automation** | Let scheduled tasks and visual workflows carry repetitive work with model selection, retries, timeouts, failure policies, run history, and notifications. |
| **Two-way channels** | Connect Feishu and personal Weixin with per-channel reply models, workspace routing, attachments, and reusable notification templates. |
| **Desktop app** | Use a native Electron window with single-instance behavior, branded icons, in-app release notes, and GitHub Releases update support. |
| **Security boundaries** | Per-session `Read only / Workspace / Full access` execution modes, one-shot approval for out-of-workspace actions, server-side secret redaction, and local user data stored outside the repository. |

> **Local sandbox note:** The default Workspace mode uses the open-source [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) to restrict Shell filesystem writes, credentials, and network access. On Windows, first use requires one UAC prompt to install a low-privilege account and network isolation; the Agent itself does not receive administrator privileges. Multiple sessions in the same workspace can run concurrently, while different workspaces safely queue a single-root grant switch to prevent cross-project writes by the shared sandbox account and repeated deny-ACL propagation across large directory trees. If sandbox initialization fails, Vesper blocks execution instead of silently falling back to unrestricted host access. The runtime is currently a Beta Research Preview: it reduces risk, but cannot make unknown code absolutely safe.

---

<a id="architecture"></a>

## ✦ Code Map

```text
vesper/
├─ .github/              # CI, release notes, and cross-platform releases
├─ docs/                 # Documentation, screenshots, and brand assets
├─ electron/             # Electron main process and secure preload
├─ public/               # Public static assets
├─ scripts/              # Icon generation, packaging, and release scripts
├─ shared/               # Workflow graph logic shared by client and server
├─ server/
│  ├─ http/              # HTTP API, SSE, and static responses
│  ├─ prompts/           # Agent system prompts and runtime identity
│  ├─ runtime/           # Pi Agent session and model runtime
│  ├─ security/          # Credential and output redaction
│  ├─ services/          # Channels, memory, workflows, and integrations
│  ├─ storage/           # Local persistence helpers
│  ├─ tests/             # Node.js test suites
│  └─ tools/             # Built-in and application tool registry
└─ src/
   ├─ app/               # Routing, navigation, branding, and localization
   ├─ assets/            # Frontend static assets
   ├─ components/        # Shared React components
   ├─ features/          # Feature pages and interactions
   ├─ hooks/             # Shared React hooks
   └─ lib/               # API and formatting utilities
```

---

<a id="start"></a>

## ✦ Begin Here

### Download the Desktop App

No source build is required. Visit the [latest GitHub Release](https://github.com/ling-kong-ran/vesper/releases/latest) to download an installer for Windows, macOS, or Linux.

> [Download the latest Vesper desktop installer →](https://github.com/ling-kong-ran/vesper/releases/latest)

### Run from Source

The following requirements apply only when developing Vesper, running the Web app, or packaging the desktop app yourself.

#### Prerequisites

- Node.js 20 or later
- npm
- At least one supported model provider and API key

#### Install Dependencies

```bash
git clone https://github.com/ling-kong-ran/vesper.git
cd vesper
npm install
```

### Web

Start the development server:

```bash
npm run dev
```

When the service is ready, the terminal prints its URL clearly. Open `http://127.0.0.1:5173` in a browser. Vesper does not create a browser tab by default; set `VESPER_OPEN_BROWSER=1` if you want it to open automatically.

On startup, the Web app compares its current Git commit with GitHub `main` rather than waiting for a Release tag. If remote commits have not been synced, the number of missing commits appears at the bottom of the left navigation; click it to review the difference. The check is informational only: it never forces a refresh, downloads files, or overwrites local source code.

Commit or stash local changes before updating the source, then run:

```bash
git pull
npm install
```

Create and run a production Web build:

```bash
npm run build
npm start
```

### Desktop

Start or package the desktop app:

```bash
npm run desktop:dev
npm run desktop:pack
```

The desktop app checks for updates shortly after startup and shows new-version status in the left navigation. Updates are never downloaded automatically; the user chooses when to begin and can review the release notes first.

To publish a release, the script updates and commits `package.json` and `package-lock.json`, creates a Git tag, then lets GitHub Actions generate release notes and publish Windows, macOS, and Linux packages:

```bash
npm run release -- patch
# minor, major, or an explicit version such as 1.2.0 are also supported
```

### Local Data

Vesper stores configuration, conversations, memory, and runtime data in:

```text
~/.vesper/agent
```

Set `VESPER_AGENT_DIR` to use another location.

### Verification

```bash
npm run lint
npm test
npm run build
```

The test suite uses the Node.js built-in test runner and lives under `server/tests/`.

---

<a id="contributing"></a>

## ✦ Walk with Vesper

Issues and pull requests are welcome:

- [Report an issue](https://github.com/ling-kong-ran/vesper/issues)
- [Open a pull request](https://github.com/ling-kong-ran/vesper/pulls)
- [View contributors](https://github.com/ling-kong-ran/vesper/graphs/contributors)

Before submitting a change, run the lint, test, and build commands above. Do not commit API keys, bot credentials, local session data, or any files from `~/.vesper/agent`.

---

<a id="acknowledgements"></a>

## ✦ Acknowledgments · Where the Light Comes From

Vesper is built on the [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) runtime and illuminated by open-source projects including Node.js, React, Electron, Vite, and i18next.

Thanks to contributors:

- [@mik-myp](https://github.com/mik-myp) — Frontend architecture modernization and i18n refactor ([#1](https://github.com/ling-kong-ran/vesper/pull/1))

<p align="right"><a href="#top">Back to top ↑</a></p>
