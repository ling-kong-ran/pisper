# ZCode visual branch

This branch keeps the release Runtime, Web/TUI protocol and storage model. The reference application's backend is not imported.

## Presentation and compatibility

- Fixed 236px sidebar with new-task/search/workflows/assets navigation and flat recent projects/sessions.
- One conversation header, time-aware ZCode empty canvas, model and thinking level in one popover; Plan/execution and approval controls remain alongside it.
- Split selection and the legacy global status bar are not exposed. The native terminal stays mounted when closed; the upper-right icon and its shortcut open it (no duplicate sidebar entry).
- Session organization, history, context/plan/file changes, attachments, approvals, streaming, cancellation and optional backend tools still use release services. Prompt suggestions move into the optional tools tray.
- Sidebar menus and suggestions stay lazy to preserve startup budgets; no new global feature CSS.
- New-task navigation uses the persisted chat creation request, not a delayed invocation of the previous page's primary action.
- Adapted artwork/layout attribution is in `THIRD_PARTY_NOTICES.md` and `LICENSES/ZCode-Apache-2.0.txt`.

- A single Pisper brand button opens/closes navigation. Closed off-canvas navigation is inert; the brand moves to the conversation header. Directory selection and conversation rename live at the upper left, not in the composer.
- Windows uses one integrated header with minimize/maximize/close controls. Terminal, context and session-tree actions use compact upper-right icons.
- The composer is capped at 600px; secondary usage/voice controls are collapsed by default. Reasoning uses a keyboard-accessible, backend-defined blue stepped slider in the model popover.
- The context panel supports up to 12 independently closable pages (files, plan, browser). Per-session drafts survive closing/reopening within the app session; browsing URLs are not written to persistent storage.
- The lower-left P avatar opens model settings; the gear opens appearance settings. Model configuration is no longer duplicated in the settings navigation. The send/stop button is 32px and the model/effort popover is 224px wide with a 24px slider thumb.
- During generation, model or thinking selections are staged without mutating the active backend run. A route-independent queue waits for backend idle and applies the latest choice once; switching sessions or visiting workflows/settings does not discard it. Closing/reloading the entire app cancels unapplied in-memory choices. A pending new model hides the old model’s thinking levels until its actual capabilities arrive. Saves block a new send without clearing the draft; failures restore the actual selection and display an error, without retrying writes.
- Model settings use a connection list and inline details/model views. Existing key preservation, enable/default/clone/delete, discovery and model management still use release APIs.

## Verification

Use Node 24 and the locked dependencies: `npm run check`, `npm test`, `npm run build`, then `npm run test:ui`.

The full UI test creates its own isolated runtime and local SSE model fixture, never reading real credentials. It covers compact control dimensions, avatar/settings navigation, model/effort persistence, active-run preselection, cancellation, route transitions, save failures/draft retention, approval/Plan controls, IME, responsive overflow, theme persistence, context pages, streaming/cancellation, background sessions/drafts, history CRUD, all release feature routes, sidebar collapse and settings-page layout transfer. Each fixture provider exposes its own model catalog, so visiting settings and refreshing models is included. Provider discovery and GitHub update checks are explicitly stubbed; a 503 model-save failure is explicitly injected to verify rollback; a real-model/native acceptance is still required. Reports/screenshots are written to a new temporary directory.

Run `cargo test --manifest-path src-tauri/Cargo.toml` for desktop bridge/transport coverage. If Windows system proxy settings intercept loopback, set process-local `NO_PROXY=127.0.0.1,localhost,::1` for this test run; do not alter the system proxy or disable certificate verification.

Start a production runtime with separate empty `PISPER_AGENT_DIR` and `PISPER_WORKSPACE_DIR`, loopback binding and automatic browser launch disabled. Set `PISPER_SMOKE_ISOLATED=1` and `PISPER_SMOKE_BASE_URL`, then run `node scripts/smoke-zcode-workbench.mjs`.

The smoke uses installed Edge by default. `PISPER_SMOKE_BROWSER` selects another Chromium executable. `PISPER_SMOKE_OUTPUT` sets the screenshot/report directory (otherwise temporary). Use a fresh empty session for welcome-canvas checks. The smoke does not call a model or copy credentials; it uses actual production assets and API responses.

For live-provider acceptance, provision only an explicitly authorized provider in the isolated agent directory. Use synthetic fixtures and verify streamed agent text, persistence/reload, real read tool, write blocked before approval, allow/reject effects on disk, plan tools and panel, thinking persistence, full-access confirmation and restoration to approval mode, stop followed by a new turn, and attached fixture reading. Never log/commit credentials; remove temporary credential copies after stopping the runtime.

Source guards changed only where this branch intentionally replaces release presentation. Runtime tests remain unchanged. Greeting boundaries/sizing are tested in `runtime/tests/zcode-workbench.test.mjs`.

## Local packaging and rollback

Push the reviewed commit, then build SEA, run SEA smoke, stage TUI and package Tauri from that same clean commit. A short Windows `subst` path may be needed by NSIS. Do not loosen bundle budgets or compatibility checks. Version manifests remain governed by the release process; same-version visual rebuilds are identified by Git revision and installer SHA-256.

Back up the installation and preserve personal agent/WebView data. Verify the actual installed executable path/hash, effective component versions, frontend asset hashes and native WebView2 conversation. Verify the normal user-data launch separately from the isolated smoke launch. Do not mistake a preview, old installer or cached component override for the installed app. Rollback uses the preserved program/installer; no release storage schema migration is introduced.

Microphone hardware, externally authenticated plugins/services, non-Windows native platforms and clean-machine WebView installation need separate device/account acceptance; they are not established by browser smoke.


### 本轮配置与会话竞态审核

- 模型目录后台刷新使用本地修订号保护，不再用保存前的迟到响应覆盖已经保存的新配置。UI 回归通过延迟真实目录响应复现旧问题，同时断言表单和连接标题（不能仅检查不受 props 更新的输入草稿）。
- 新连接创建与批量追加模型是两个独立后端写入。前端保留服务端归一化后的已创建 ID，追加失败后重试走更新接口，取消仍显示已经持久化的连接；不会把部分成功当成完全回滚。仅影响 Web 配置编辑，不改变 Runtime/TUI 协议或凭据存储。
- 配置写入进行中禁用对话框关闭入口，避免操作尚未完成时误以为取消已回滚；失败后可正常取消或重试。
- 未发送草稿按会话保留于当前页面内存，切换会话不丢失；整页重新加载不承诺保留。UI 回归分别测试会话切换和持久化后端偏好的 reload，避免混淆两种生命周期。
- 注入的模型保存与批量追加失败都有精确路径/状态断言；所有测试中暂扣的响应均有超时，不允许无界等待掩盖失败。
