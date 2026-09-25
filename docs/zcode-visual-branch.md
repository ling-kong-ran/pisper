# ZCode visual branch

This branch keeps the release Runtime, Web/TUI protocol and storage model. The reference application's backend is not imported.

## Presentation and compatibility

- Fixed 236px sidebar with new-task/search/automation/plugin navigation and flat recent projects/sessions.
- One conversation header, time-aware ZCode empty canvas, model and thinking level in one popover; Plan/execution and approval controls remain alongside it.
- Split selection and the legacy global status bar are not exposed. The native terminal stays mounted when closed; the upper-right icon, More tools and its shortcut open it.
- Session organization, history, context/plan/file changes, attachments, approvals, streaming, cancellation and optional backend tools still use release services. Prompt suggestions move into the optional tools tray.
- Sidebar menus and suggestions stay lazy to preserve startup budgets; no new global feature CSS.
- New-task navigation uses the persisted chat creation request, not a delayed invocation of the previous page's primary action.
- Adapted artwork/layout attribution is in `THIRD_PARTY_NOTICES.md` and `LICENSES/ZCode-Apache-2.0.txt`.

- A single Pisper brand button opens/closes navigation. Closed off-canvas navigation is inert; the brand moves to the conversation header. Directory selection and conversation rename live at the upper left, not in the composer.
- Windows uses one integrated header with minimize/maximize/close controls. Terminal, context and session-tree actions use compact upper-right icons.
- The composer is capped at 600px; secondary usage/voice controls are collapsed by default. Reasoning uses a keyboard-accessible, backend-defined blue stepped slider in the model popover.
- The context panel supports up to 12 independently closable pages (files, plan, browser). Per-session drafts survive closing/reopening within the app session; browsing URLs are not written to persistent storage.
- Model settings use a connection list and inline details/model views. Existing key preservation, enable/default/clone/delete, discovery and model management still use release APIs.

## Verification

Use Node 24 and the locked dependencies: `npm run check`, `npm test`, `npm run build`, then `npm run test:ui`.

The full UI test creates its own isolated runtime and local SSE model fixture, never reading real credentials. It covers model/effort persistence, approval/Plan controls, IME, responsive overflow, theme persistence, context pages, streaming/cancellation, background sessions/drafts, history CRUD, all release feature routes, sidebar collapse and settings-page layout transfer. Provider discovery and GitHub update checks are explicitly stubbed; a real-model/native acceptance is still required. Reports/screenshots are written to a new temporary directory.

Run `cargo test --manifest-path src-tauri/Cargo.toml` for desktop bridge/transport coverage. If Windows system proxy settings intercept loopback, set process-local `NO_PROXY=127.0.0.1,localhost,::1` for this test run; do not alter the system proxy or disable certificate verification.

Start a production runtime with separate empty `PISPER_AGENT_DIR` and `PISPER_WORKSPACE_DIR`, loopback binding and automatic browser launch disabled. Set `PISPER_SMOKE_ISOLATED=1` and `PISPER_SMOKE_BASE_URL`, then run `node scripts/smoke-zcode-workbench.mjs`.

The smoke uses installed Edge by default. `PISPER_SMOKE_BROWSER` selects another Chromium executable. `PISPER_SMOKE_OUTPUT` sets the screenshot/report directory (otherwise temporary). Use a fresh empty session for welcome-canvas checks. The smoke does not call a model or copy credentials; it uses actual production assets and API responses.

For live-provider acceptance, provision only an explicitly authorized provider in the isolated agent directory. Use synthetic fixtures and verify streamed agent text, persistence/reload, real read tool, write blocked before approval, allow/reject effects on disk, plan tools and panel, thinking persistence, full-access confirmation and restoration to approval mode, stop followed by a new turn, and attached fixture reading. Never log/commit credentials; remove temporary credential copies after stopping the runtime.

Source guards changed only where this branch intentionally replaces release presentation. Runtime tests remain unchanged. Greeting boundaries/sizing are tested in `runtime/tests/zcode-workbench.test.mjs`.

## Local packaging and rollback

Push the reviewed commit, then build SEA, run SEA smoke, stage TUI and package Tauri from that same clean commit. A short Windows `subst` path may be needed by NSIS. Do not loosen bundle budgets or compatibility checks. Version manifests remain governed by the release process; same-version visual rebuilds are identified by Git revision and installer SHA-256.

Back up the installation and preserve personal agent/WebView data. Verify the actual installed executable path/hash, effective component versions, frontend asset hashes and native WebView2 conversation. Verify the normal user-data launch separately from the isolated smoke launch. Do not mistake a preview, old installer or cached component override for the installed app. Rollback uses the preserved program/installer; no release storage schema migration is introduced.

Microphone hardware, externally authenticated plugins/services, non-Windows native platforms and clean-machine WebView installation need separate device/account acceptance; they are not established by browser smoke.
