---
name: screenshot-studio
description: Regenerate the Pisper product screenshots under docs/shots/ against the current UI. Starts an isolated dev instance, seeds fictional demo data through the real runtime APIs, captures every referenced page at the exact 2558x1380 asset size, and replaces the files. Use only when the user asks to refresh product screenshots or when the docs gallery no longer matches the UI.
disable-model-invocation: true
allowed-tools: read grep find ls edit write bash
---

# Screenshot Studio

Refresh the screenshots in `docs/shots/` so the product site and README previews match the current UI. Do not change product code; only capture and replace images (and the references that name them, when needed).

## When to use

Invoke only when the user explicitly asks:

```text
/skill:screenshot-studio
```

This skill is for screenshot refresh work only. Do not run it during ordinary implementation or review.

## Output invariants

- Every Web screenshot is exactly **2558x1380** (viewport `1279x690` at `deviceScaleFactor 2`).
- `welcome-dark.png` is the only dark-theme shot; every other page is light theme.
- `cli.png` and `pisper-demo.gif` are TUI/demo assets; leave them untouched unless the TUI itself changed.
- No real user data, provider keys, or machine paths may appear. Use only fictional demo content and repository-relative paths.
- The isolated instance must never read the user's `~/.pisper/agent` or the port-5173 dev server state.

## Workflow

### 1. Start an isolated dev instance

```bash
node .pisper/skills/screenshot-studio/scripts/start-isolated-server.mjs --reset
```

This stops any prior Skill-owned process, clears only the configured screenshot agent/run directories, starts `runtime/index.mjs`, and waits for `/api/health`. Defaults are port `5180` and repository-relative directories under `generated/`; no project path is hard-coded.

### 2. Seed fictional demo data

```bash
node .pisper/skills/screenshot-studio/scripts/seed-demo-data.mjs
```

Creates through the real runtime APIs (never by editing the UI): six sessions, three with injected conversation transcripts, three empty; link + generated-image assets; a memory space with nodes; schedules; workflows (one published); and a configured OpenAI-compatible provider. Writes the created ids to `generated/screenshot-run/state.json`.

### 2b. Restart the isolated instance

```bash
node .pisper/skills/screenshot-studio/scripts/start-isolated-server.mjs
```

Run it again after seeding. API-created sessions live in memory only; restarting drops the pending entries so the JSONL transcripts become the single source of truth (message counts and conversation text).

### 3. Capture every page

```bash
node .pisper/skills/screenshot-studio/scripts/capture-screenshots.mjs
```

Visits every route referenced by `docs/index.html` and `docs/show.html`, applies the localStorage presets, opens the split dock via the real tab context menu, selects the memory space, and saves the PNGs to the configured run directory. The desktop terminal shot uses the real `TerminalPanel` with a screenshot-only bridge and deterministic fictional output; it never starts a host shell.

### 4. Verify and replace

```bash
node .pisper/skills/screenshot-studio/scripts/verify-screenshots.mjs
```

Parses each PNG's IHDR directly in Node, asserts every expected shot exists at `2558x1380`, and copies nothing until the full set passes. It then replaces the configured docs shot directory. Confirm `docs/index.html` width/height attributes still match. Do not commit without the user's request.

### 5. Stop the isolated instance

```bash
node .pisper/skills/screenshot-studio/scripts/start-isolated-server.mjs --stop
```

The PID file belongs only to this Skill. Do not kill unrelated processes by port.

## Hard-won constraints

1. **Provider must be configured first.** `App.tsx` auto-creates a new chat session and redirects to `/config` when `/api/config` reports no usable provider. `saveConfig` reads the API key from the **top-level** `apiKey` field:

   ```json
   { "provider": "openai", "model": "gpt-5", "apiKey": "sk-demo", "defaultProvider": "openai", "defaultModel": "gpt-5" }
   ```

   Setting `configured: true` inside a `providers` array is ignored.

2. **Empty sessions need a transcript file.** `POST /api/sessions` only keeps the session in memory; the JSONL file is written on the first message. After a restart, sessions without a JSONL disappear from the list. For empty demo sessions, write a minimal file `<iso-timestamp>_<sessionId>.jsonl` in the agent `sessions/` dir containing the `session` header plus one `session_info` entry.

3. **Session transcripts use pi v3 JSONL.** Events chain with `id`/`parentId`; assistant content blocks use `thinking` + `text` types. Completed thinking renders collapsed by default in the UI (correct behavior).

4. **The browser must block provider discovery.** Route-intercept `**/api/providers/discovery` with `{"providers":[],"errors":[]}` so the isolated instance never scans real Codex/Claude config files.

5. **Hash router.** Routes are `/#/chat`, `/#/config`, etc.

6. **Dock split uses real UI.** The split layout cannot be reliably injected via localStorage. Open `pisper-tiled-sessions` with two session ids, then right-click the second tab (`.dv-tab`) and click the `.dv-context-menu-item` labeled `拆分到右侧`.

7. **localStorage presets** (set before first navigation):
   - `pisper-ui` = Zustand persist payload with `state.theme` set to `light` (or `dark` for `welcome-dark.png`), `state.sidebarCollapsed` set to `false`, and `state.density` set to `comfortable`
   - `pisper-theme` = the same theme for legacy migration compatibility
   - `pisper-language` = `zh-CN`
   - `pisper-sidebar-collapsed` = `false`
   - `pisper-active-session` = the session to open
   - `pisper-tiled-sessions` = `[]` (or the two ids for the split)
   - remove `pisper-chat-dock-layout-v1`
   - reload after changing the theme because hash navigation alone does not recreate the persisted UI store

8. **Browser discovery.** Playwright tries installed Edge and Chrome channels, then bundled Chromium. Set `SCREENSHOT_BROWSER_PATH` for an explicit executable or `SCREENSHOT_BROWSER_CHANNEL` for another Chromium channel; never add a machine-specific executable path to the script.

9. **Portable configuration.** `screenshot-config.mjs` owns all paths and network defaults. Optional overrides are `SCREENSHOT_PORT`, `SCREENSHOT_HOST`, `SCREENSHOT_BASE_URL`, `SCREENSHOT_AGENT_DIR`, `SCREENSHOT_RUN_DIR`, `SCREENSHOT_SHOTS_DIR`, and `SCREENSHOT_WORKSPACE_DIR`. Relative path overrides resolve from the repository root.

10. **Viewport math.** `1279x690` × `deviceScaleFactor 2` = `2558x1380`, matching every existing asset and the `width`/`height` attributes in `docs/index.html`.

## Verification expectations

- All 22 Web shots replaced; `cli.png` + `pisper-demo.gif` untouched.
- Every file exactly `2558x1380`.
- `terminal.png` shows the real desktop terminal panel bound to the active chat session, with only fictional output and repository-relative labels.
- `git status` shows screenshot assets, intentional docs references, and Skill maintenance changes only; configured run/agent directories remain gitignored.
- Product source remains unchanged. Report changed references in `docs/index.html` / `docs/show.html`.
