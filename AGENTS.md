# AGENTS.md

Guidance for coding agents working in the **Pisper** repository.

## Project

Pisper is a local-first multi-agent workspace built on [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent). It ships:

- a React web UI (dev server + production static assets)
- a Node.js app server / desktop sidecar that hosts sessions, tools, MCP, skills, memory, workflows, channels, and schedules
- a Tauri 2 desktop shell with a system WebView and a Node SEA sidecar
- a Rust Ratatui TUI client

Requires **Node.js 20+** (desktop SEA packaging docs target Node 24). Runtime agent data defaults to `~/.pisper/agent` (`PISPER_AGENT_DIR` overrides). Do not commit API keys, bot credentials, or personal data from that directory.

## Architecture

| Path | Role |
| --- | --- |
| `src/` | React + TypeScript frontend (Vite, Tailwind 4, shadcn/ui, Zustand, i18next) |
| `src/app/` | App shell: routing, providers, i18n wiring, navigation |
| `src/features/` | Feature pages (chat, config, skills, MCP/plugins, workflows, …) |
| `src/components/ui/` | shadcn/ui primitives; prefer composing these over one-off styles |
| `src/locales/{zh-CN,en-US}/` | Translation namespaces (`namespace:key`) |
| `server/` | Node app server (plain ESM `.mjs`): HTTP API, runtime, services, tools |
| `server/runtime/` | Pi agent runtime integration |
| `server/services/` | Domain services (sessions, MCP, schedules, workflows, providers, …) |
| `server/tools/app/` | Application-level agent tools (one module per tool) |
| `server/tests/` | Node test suite (`tsx --test`) |
| `shared/` | Small JS modules shared by server and client (e.g. workflow graph, release notes) |
| `src-tauri/` | Tauri 2 shell (desktop bridge, pet window, updater, CLI install) |
| `src-tui/` | Rust TUI (`pisper` CLI) |
| `scripts/` | SEA packaging, release, smoke tests, i18n check |
| `docs/` | Product docs, screenshots, sponsor config, packaging notes |
| `dist/` | Vite production build output |
| `release/` | Packaged SEA / TUI / Tauri artifacts (gitignored) |

**Runtime shape (desktop):** Tauri shell → `pisper-sidecar` (Node SEA) → `sidecar-runtime/` (server, Pi packages, skills, native modules). Dev web flow: `server/index.mjs` embeds Vite middleware and serves the SPA + API on `127.0.0.1:5173` by default.

Path aliases: `@/*` → `src/*`, `@shared/*` → `shared/*`.

## Commands

```bash
# Setup
npm install                 # install dependencies from package-lock.json
npm ci                      # reproducible clean install for CI/release

# Day-to-day web server
npm run dev                 # web + API with Vite middleware
npm run build               # production frontend build and bundle-budget check
npm run preview             # serve built assets through the production server
npm start                   # alias for preview

# Quality
npm run typecheck           # TypeScript checks for src, node, and JS-check config
npm run lint                # oxlint
npm run format              # prettier --write .
npm run format:check        # verify Prettier formatting
npm run i18n:check          # verify literal src keys exist in zh-CN and en-US
npm run check               # typecheck + lint + i18n:check + format:check
npm test                    # all server/tests/*.test.mjs tests
npx tsx --test server/tests/foo.test.mjs  # run one or more focused tests

# Node SEA sidecar
npm run sidecar:dev         # run the sidecar directly in development
npm run sidecar:sea         # build the Node SEA and stage its runtime closure
npm run sidecar:sea:smoke   # smoke-test the staged SEA/runtime/API

# Tauri desktop (requires Rust and platform Tauri dependencies)
npm run desktop:webview:dev       # build frontend and launch tauri dev
npm run desktop:webview:smoke -- http://127.0.0.1:9223  # smoke-test a running WebView CDP endpoint
npm run desktop:webview:package   # package the Tauri desktop application
npm run desktop:webview:build     # SEA + SEA smoke + TUI stage + desktop package

# Rust TUI
npm run tui:dev             # cargo run the TUI
npm run tui:check           # cargo check
npm run tui:test            # cargo test
cargo fmt --manifest-path src-tui/Cargo.toml -- --check  # verify Rust formatting
npm run tui:stage           # build and stage the TUI with the SEA sidecar
npm run tui:package         # build and package the TUI distribution
npm run tui:build           # SEA build followed by TUI packaging

# Versioning and release
npm run version             # synchronize src-tui/Cargo.toml and Cargo.lock to package.json
npm run release -- patch    # refresh dependencies, run release gates, bump version, tag, and push
npm run release -- minor
npm run release -- major
npm run release -- 0.4.20
```

Prefer `npm run check` and `npm test` before considering a change done. Run desktop/TUI packaging only when touching those surfaces.

## Conventions

### Frontend (`src/`)

- TypeScript strict; no unused locals/parameters.
- Import with `@/` aliases. Under `src/**`, oxlint enforces **no relative parent imports** (`import/no-relative-parent-imports`).
- UI: Tailwind + shadcn (`components.json` style `radix-nova`, icons via `lucide-react`). Use `cn()` from `@/lib/utils`.
- Feature code lives under `src/features/<area>/`; shared layout/chrome under `src/components/`.
- i18n: `t('namespace:key')` / `translateText('namespace:key')` with **string-literal** keys only. Both `zh-CN` and `en-US` must define every key; no Chinese characters as keys. Run `npm run i18n:check` after UI copy changes.
- Prettier: single quotes, no semicolons, trailing commas, print width 100. Note: Prettier currently ignores most of `server/`, `scripts/`, and `shared/` (see `.prettierignore`); still match nearby file style.

### Server (`server/`)

- ESM `.mjs` modules; keep API handlers in `server/http/`, domain logic in `server/services/`, Pi wiring in `server/runtime/`.
- App tools: one module under `server/tools/app/` exporting `manifest` and a `create…Tool(context)` factory using `defineTool()`. Register in `server/tools/app/index.mjs`. Factories take `cwd`/service deps—do not couple tools directly to `AgentRuntimeService`.
- Tests are colocated as `server/tests/*.test.mjs`. Prefer extending existing service/runtime tests when behavior changes.

### Shared / desktop / TUI

- Pure shared logic that both UI and server need goes in `shared/` (with `.d.mts` types when consumed from TS).
- Desktop packaging and updater details: `docs/node-sea-webview.md`. Do not reintroduce Electron packaging paths.
- TUI user-facing docs: `src-tui/README.md` / `README.en.md`. Keep version in sync via `npm run version` / release scripts when cutting releases.

## Verification expectations

1. For TypeScript/UI work: `npm run check` (or at least `typecheck` + `lint` + `i18n:check` when touching strings).
2. For server/runtime/tool/service changes: `npm test` (or the relevant test file under `server/tests/`).
3. For TUI Rust changes: `npm run tui:check` and `npm run tui:test`.
4. For SEA/desktop packaging changes: `npm run sidecar:sea:smoke` (and platform-specific packaging only with required Rust/Tauri toolchain).
5. Do not invent new top-level package managers or dual lockfiles; this repo uses **npm** (`package-lock.json`).

## Out of scope / safety

- Do not commit secrets, `release/` artifacts, `src-tauri/binaries/`, or user agent state under `.pisper/`.
- Do not target Electron; desktop is Tauri + Node SEA only.
- Public sponsor placement config is `docs/sponsors.json`—keep it free of user session/provider credentials.
