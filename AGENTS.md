# AGENTS.md

Guidance for coding agents working in the **Pisper** repository.

## Project

Pisper is a local-first multi-agent workspace built on [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent). It ships:

- a React web UI (dev server + production static assets)
- a Node.js app runtime / desktop sidecar that hosts sessions, tools, MCP, skills, memory, workflows, channels, and schedules
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
| `runtime/` | Node app runtime (plain ESM `.mjs`): HTTP API, agent integration, services, tools |
| `runtime/runtime/` | Pi agent runtime integration |
| `runtime/services/` | Domain services (sessions, MCP, schedules, workflows, providers, …) |
| `runtime/tools/app/` | Application-level agent tools (one module per tool) |
| `runtime/tests/` | Node test suite (`tsx --test`) |
| `packages/pisper/` | Private source manifest and npm installer/launcher published as `pisper` |
| `shared/` | Small JS modules shared by runtime and client (e.g. workflow graph, release notes) |
| `src-tauri/` | Tauri 2 shell (desktop bridge, pet window, updater, CLI install) |
| `src-tui/` | Rust TUI (`pisper` CLI) |
| `scripts/` | SEA packaging, release, smoke tests, i18n check |
| `docs/` | Product docs, screenshots, sponsor config, packaging notes |
| `dist/` | Vite production build output |
| `release/` | Packaged SEA / TUI / Tauri artifacts (gitignored) |

**Runtime shape (desktop):** Tauri shell → `pisper-sidecar` (Node SEA) → `sidecar-runtime/` (Pisper runtime, Pi packages, skills, native modules). Dev web flow: `runtime/index.mjs` embeds Vite middleware and serves the SPA + API on `127.0.0.1:5173` by default.

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
npm test                    # all runtime/tests/*.test.mjs tests
npx tsx --test runtime/tests/foo.test.mjs  # run one or more focused tests

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

# npm installer package
npm run npm:pack                  # build the lightweight pisper tarball
npm run npm:pack:check            # build and validate tarball contents and behavior

# Versioning and release
npm run release -- patch          # auto-detect and publish every changed component
npm run release -- 0.4.31         # explicit version for every detected component
```

Prefer `npm run check` and `npm test` before considering a change done. Run desktop/TUI packaging only when touching those surfaces.

### Release policy (agents)

Releases must ship **substantive product changes**. Do **not** cut a version when the only delta since the latest component tag is version metadata, dependency refresh, formatting, docs-only nits, or other release-script bookkeeping.

Desktop, TUI, runtime, and npm releases have independent versions and tags. Desktop uses `src-tauri/desktop-package.json` with `vX.Y.Z`; TUI uses `src-tui/Cargo.toml` with `tui-vX.Y.Z`; runtime/web uses root `package.json` with `runtime-vX.Y.Z`; the `pisper` npm manifest uses `packages/pisper/package.json` with `npm-vX.Y.Z`. Only desktop Releases are marked as GitHub `latest` and publish signed `latest.json` updater metadata. TUI Releases keep a self-contained distribution and add a thin TUI-only updater archive; runtime Releases ship the SEA/runtime component. Every component archive is minisign-signed, while TUI builds never build or sign Tauri installers.

The root package stays private. npm publishes only the installer/launcher package named `pisper`, whose installation downloads and verifies the selected signed TUI and Runtime components without the Desktop frontend. Do not publish the root Runtime package directly to npm, bundle release archives into the npm tarball, or copy Runtime into the TUI installation. The launcher must reuse the single standard component Runtime via `PISPER_SIDECAR_PATH` and `PISPER_APP_ROOT`.

Before running `npm run release -- <version>`:

1. Confirm you are on the `release` branch, the tracked working tree is clean, and local `release` exactly matches `origin/release`.
2. Inspect `git log --oneline <latest-tag>..HEAD` and `git diff --stat <latest-tag>..HEAD`.
3. Require at least one substantive commit since the latest tag for each detected component (`v*`, `tui-v*`, or `runtime-v*`): `feat`, `fix`, `perf`, user-facing behavior, security, or packaging that changes shipped artifacts. Pure `chore(deps)`, `chore(release)`, `style`, and docs-only commits do **not** count by themselves.
4. If there is nothing substantive to ship, **stop**. Do not invent a patch release, do not run `npm run release` “just to push”, and do not force-publish after dependency refresh alone.
5. When the user asks to “发布新版本” but HEAD is already the applicable components' release commit / tag with no later product commits, report that the latest version is already published and wait for new work.

Once `npm run release` dispatches a component, treat the remote `release` branch as frozen until the command exits and **every** selected component workflow has completed. Before committing or pushing to `release`, check `.github/workflows/release.yml` for queued or in-progress runs and account for later components in the same local release command; do not resume after only the first component finishes. Keep follow-up work uncommitted or on another branch during this window. After finalization, fetch `origin/release` and tags, fast-forward local `release`, verify that no release run remains active, and only then commit and push queued changes. Advancing `release` during this window can invalidate the workflow's atomic branch-advance check.

`npm run release` enforces this gate in `scripts/release.mjs` via `scripts/release-policy.mjs`. It always compares each component with its own latest tag, dispatches every component with substantive owned changes, and runs the union of their local checks once. Manual component scopes are rejected so a release cannot accidentally omit another changed component. Each dispatch passes the component, exact source SHA, and target version to `.github/workflows/release.yml`; the local script must **not** bump versions, create tags, or push release metadata. Multi-component workflows share the immutable source SHA and run in one global queue; later jobs may advance only across validated `chore(release-<component>)` commits that touch exactly that component's version files. GitHub Actions stages only that component's version files in artifacts, verifies and builds its platform packages, validates the exact asset set, then lets `github-actions[bot]` commit `chore(release-<component>): <tag>` and atomically push the `release` branch plus tag immediately before publishing a Draft Release. Any earlier failure leaves the remote version and tag unchanged; finalization failures run compensating cleanup. Do not reintroduce tag-push-triggered releases.

When a detected component is **Runtime** or **TUI**, `npm run release` automatically chains an npm release: it derives `pisper@<next>` from the manifest using the same `major|minor|patch` bump, or uses the same explicit `X.Y.Z` version as the selected components, passes the exact new TUI/Runtime versions, and runs `npm run npm:pack:check` locally before dispatching. The npm publish happens inside the first npm-related component workflow via `.github/workflows/publish-npm.yml` (`workflow_call` with `npm_version`, `tui_version`, `runtime_version`, `source_sha`); npm is never dispatched twice for the same release. The workflow commits only `packages/pisper/package.json` as `chore(release-npm): npm-vX.Y.Z`. npm publishing uses the repository's Trusted Publisher connection, GitHub OIDC, and provenance; do not add registry tokens or pass inherited Secrets into that workflow. Never commit registry credentials. There is no manual per-component or standalone npm release command — component and npm releases are always driven by automatic detection.

## Conventions

- **注释语言：** 代码注释一律使用中文。新增或修改 `src/`、`runtime/`、`shared/`、`src-tauri/`、`src-tui/` 等任何代码时，解释性注释（行注释、块注释、JSDoc/doc 注释）均须用中文书写；注释应说明「为什么」，而不是复述代码本身。

### Frontend (`src/`)

- TypeScript strict; no unused locals/parameters.
- Import with `@/` aliases. Under `src/**`, oxlint enforces **no relative parent imports** (`import/no-relative-parent-imports`).
- UI: Tailwind + shadcn (`components.json` style `radix-nova`, icons via `lucide-react`). Use `cn()` from `@/lib/utils`.
- Styling policy: use Tailwind utilities for component layout and appearance, and use or extend shadcn primitives for shared controls. Repeated utility combinations belong in a React component or typed variant, not in a new global CSS class.
- Do **not** add page-, feature-, or control-level semantic classes to `src/index.css`, including `@apply` aliases that merely hide Tailwind utilities. Keep global CSS limited to design tokens/theme variables, resets and base element rules, keyframes, and narrowly scoped third-party or complex selector overrides that Tailwind cannot express clearly.
- Treat existing global semantic classes as migration debt: when changing a component that uses them, migrate the touched styling to Tailwind/shadcn when the change can remain focused. Do not perform unrelated bulk rewrites solely to remove old classes.
- Feature code lives under `src/features/<area>/`; shared layout/chrome under `src/components/`.
- i18n: `t('namespace:key')` / `translateText('namespace:key')` with **string-literal** keys only. Both `zh-CN` and `en-US` must define every key; no Chinese characters as keys. Run `npm run i18n:check` after UI copy changes.
- Prettier: single quotes, no semicolons, trailing commas, print width 100. Note: Prettier currently ignores most of `runtime/`, `scripts/`, and `shared/` (see `.prettierignore`); still match nearby file style.

### Runtime (`runtime/`)

- ESM `.mjs` modules; keep API handlers in `runtime/http/`, domain logic in `runtime/services/`, Pi wiring in `runtime/runtime/`.
- App tools: one module under `runtime/tools/app/` exporting `manifest` and a `create…Tool(context)` factory using `defineTool()`. Register in `runtime/tools/app/index.mjs`. Factories take `cwd`/service deps—do not couple tools directly to `AgentRuntimeService`.
- Tests are colocated as `runtime/tests/*.test.mjs`. Prefer extending existing service/runtime tests when behavior changes.
- Runtime changes must stay compatible with **both** clients: the React web UI (`src/`) and the Rust TUI (`src-tui/`). Anything the runtime emits over HTTP (JSON bodies and SSE frames) must be strict, valid JSON/UTF-8 that both `JSON.parse` and `serde_json` accept — the browser tolerates lone surrogates and other lenient encodings that `serde_json` rejects and that tear down the whole TUI stream. When touching runtime wire output, run `npm test` and `npm run tui:check` / `npm run tui:test`.

### Shared / desktop / TUI

- Pure shared logic that both UI and runtime need goes in `shared/` (with `.d.mts` types when consumed from TS).
- Desktop packaging and updater details: `docs/node-sea-webview.md`. Do not reintroduce Electron packaging paths.
- TUI user-facing docs: `src-tui/README.md` / `README.en.md`. TUI versions advance only through the scoped release script; do not synchronize them to desktop or runtime versions.
- When adding or changing a TUI top-level command, subcommand, option, or Slash command, update the corresponding `--help`/command help text and its coverage in the same change.

## Verification expectations

1. For TypeScript/UI work: `npm run check` (or at least `typecheck` + `lint` + `i18n:check` when touching strings).
2. For runtime/runtime/tool/service changes: `npm test` (or the relevant test file under `runtime/tests/`).
3. For TUI Rust changes: `npm run tui:check` and `npm run tui:test`.
4. For Tauri desktop changes: `cargo test --manifest-path src-tauri/Cargo.toml` and `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`.
5. For SEA/desktop packaging changes: `npm run sidecar:sea:smoke` (and platform-specific packaging only with required Rust/Tauri toolchain).
6. For npm installer or npm release changes: `npm run npm:pack:check`, `node scripts/validate-npm-targets.mjs`, and the focused `runtime/tests/npm-cli-package.test.mjs` test.
7. Do not invent new top-level package managers or dual lockfiles; this repo uses **npm** (`package-lock.json`).

## Out of scope / safety

- Do not commit secrets, `release/` artifacts, `src-tauri/binaries/`, or user agent state under `.pisper/`.
- Do not target Electron; desktop is Tauri + Node SEA only.
- Public sponsor placement config is `docs/sponsors.json`—keep it free of user session/provider credentials.
