# Pisper visual branch archive

> Historical branch record. The current release UI and validation commands are defined by the repository source and development workflow.

This branch retains the release Runtime and existing Web/TUI protocol, with additive temporary-chat endpoints and metadata. The reference application's backend is not imported.

## Presentation and compatibility

- Desktop sidebar with Home/new-task/search/workflows/assets navigation and flat recent projects/sessions. Collapsing leaves a 64px icon rail with navigation, Settings and Expand controls; mobile keeps its full-width drawer. Home appears above New task and returns to the existing conversation, preserving its unsent draft.
- One conversation header, time-aware ZCode empty canvas, model and thinking level in one popover; Plan/execution and approval controls remain alongside it.
- Split selection and the legacy global status bar are not exposed. The native terminal stays mounted when closed; the upper-right icon and its shortcut open it (no duplicate sidebar entry).
- Session organization, history, context/plan/file changes, attachments, approvals, streaming, cancellation and optional backend tools still use release services. Prompt suggestions move into the optional tools tray.
- Sidebar menus and suggestions stay lazy to preserve startup budgets; no new global feature CSS.
- New-task navigation uses the persisted chat creation request, not a delayed invocation of the previous page's primary action.
- Adapted artwork/layout attribution is in `THIRD_PARTY_NOTICES.md` and `LICENSES/Upstream-Greeting-Apache-2.0.txt`.

- The Pisper brand is a static logo. A separate labeled Collapse control sits at the right edge beside Settings in the expanded sidebar footer. In the desktop icon rail, Settings and Expand stack vertically; the same toggle retains keyboard focus. Icon controls keep accessible names and hover labels, and collapsed project history is hidden from keyboard navigation. Directory selection and conversation rename live at the upper left, not in the composer.
- Windows uses one integrated header with minimize/maximize/close controls. Terminal, context and session-tree actions use compact upper-right icons.
- Chat layout switching is available beside the context and theme icons. It shares the settings page's built-in and saved templates, marks the current selection, and links to layout management. Switching preserves the conversation and draft; templates without a header retain a floating layout switch button.
- Header theme buttons cycle System → Dark → Light and show the current mode's icon plus the next mode in their labels. Following the OS may have the same colors as an explicit mode, but the indicator still changes. Legacy scheduled preferences remain readable and available in appearance settings; the shortcut leaves that mode for System and does not cycle back to it.
- The composer is capped at 600px; secondary usage/voice controls are collapsed by default. Reasoning uses a keyboard-accessible, backend-defined blue stepped slider in the model popover.
- The context panel supports up to 12 independently closable pages (files, plan, browser, temporary chat). Per-session drafts survive closing/reopening within the app session; browsing URLs are not written to persistent storage.
- Completion opens a closed context panel only when the current successful run left file changes. Empty replies, historical changes, reverted files and stopped runs do not open it; an already open panel keeps its selected page. Switching sessions or starting another run cancels pending automatic checks.
- A single lower-left Settings button opens the complete settings navigation, including providers, appearance, and the other settings sections. Home remains available there to return to the conversation. The send/stop button is 32px and the model/effort popover is 224px wide with a 24px slider thumb.
- During generation, model or thinking selections are staged without mutating the active backend run. A route-independent queue waits for backend idle and applies the latest choice once; switching sessions or visiting workflows/settings does not discard it. Closing/reloading the entire app cancels unapplied in-memory choices. A pending new model hides the old model’s thinking levels until its actual capabilities arrive. Saves block a new send without clearing the draft; failures restore the actual selection and display an error, without retrying writes.
- Model settings show configured or user-created connections in one list, with no unused preset catalog. The page header's Quick setup button is the single primary creation entry. Its three steps collect the endpoint, protocol, and model; the full connection dialog is reserved for cloning. A connection saved before an additional-model failure stays visible when the wizard closes, and retrying reuses it. A new or cloned connection is selected after creation. Mobile layouts show connection names in a horizontally scrollable list above the details. Existing key preservation, enable/default/clone/delete, discovery and model management still use release APIs.

## Verification

The default connection has an explicit badge independent of the selected row and enabled indicator. With no connections, the navigation rail and default summary stay hidden. First-run onboarding remains dismissible and leads directly to Quick setup; saving the first usable connection initializes the default model.

The UI suite also starts an isolated development server to verify first-run onboarding at desktop and mobile widths, dismiss/reload behavior, initial connection creation, and the channels route's lazy-loaded translations. Production-only route checks do not catch development-server JSON module MIME mismatches.

Use Node 24 and the locked dependencies: `npm run check`, `npm test`, `npm run build`, then `npm run test:ui`.

The full UI test creates its own isolated runtime and local SSE model fixture, never reading real credentials. It covers compact control dimensions, unified settings navigation, Home returning to the conversation draft, configured-provider filtering and the single Quick setup entry, model/effort persistence, active-run preselection, cancellation, route transitions, save failures/draft retention, approval/Plan controls, IME, responsive overflow, theme persistence, context pages, streaming/cancellation, background sessions/drafts, history CRUD, all release feature routes, keyboard sidebar collapse/expand with focus transfer, and settings-page layout transfer. Each fixture provider exposes its own model catalog, so visiting settings and refreshing models is included. Provider discovery and GitHub update checks are explicitly stubbed; a 503 model-save failure is explicitly injected to verify rollback; a real-model/native acceptance is still required. Reports/screenshots are written to a new temporary directory.

Run `cargo test --manifest-path src-tauri/Cargo.toml` for desktop bridge/transport coverage. If Windows system proxy settings intercept loopback, set process-local `NO_PROXY=127.0.0.1,localhost,::1` for this test run; do not alter the system proxy or disable certificate verification.

Start a production runtime with separate empty `PISPER_AGENT_DIR` and `PISPER_WORKSPACE_DIR`, loopback binding and automatic browser launch disabled. Set `PISPER_SMOKE_ISOLATED=1` and `PISPER_SMOKE_BASE_URL`, then run `node scripts/smoke-pisper-workbench.mjs`.

The smoke uses installed Edge by default. `PISPER_SMOKE_BROWSER` selects another Chromium executable. `PISPER_SMOKE_OUTPUT` sets the screenshot/report directory (otherwise temporary). Use a fresh empty session for welcome-canvas checks. The smoke does not call a model or copy credentials; it uses actual production assets and API responses.

For live-provider acceptance, provision only an explicitly authorized provider in the isolated agent directory. Use synthetic fixtures and verify streamed agent text, persistence/reload, real read tool, write blocked before approval, allow/reject effects on disk, plan tools and panel, thinking persistence, full-access confirmation and restoration to approval mode, stop followed by a new turn, and attached fixture reading. Never log/commit credentials; remove temporary credential copies after stopping the runtime.

Source guards changed only where this branch intentionally replaces release presentation. Existing Runtime protocol behavior remains compatible; temporary-chat metadata and provider model exclusions are additive. Frontend regression tests cover greeting boundaries/sizing, provider selection, and asynchronous context-panel checks under `runtime/tests/`.

## Local packaging and rollback

Push the reviewed commit, then build SEA, run SEA smoke, stage TUI and package Tauri from that same clean commit. A short Windows `subst` path may be needed by NSIS. Do not loosen bundle budgets or compatibility checks. Version manifests remain governed by the release process; same-version visual rebuilds are identified by Git revision and installer SHA-256.

Back up the installation and preserve personal agent/WebView data. Verify the actual installed executable path/hash, effective component versions, frontend asset hashes and native WebView2 conversation. Verify the normal user-data launch separately from the isolated smoke launch. Do not mistake a preview, old installer or cached component override for the installed app. Rollback uses the preserved program/installer; the temporary-chat rollback steps are documented in [the architecture decision](architecture/side-chat.md).

Microphone hardware, externally authenticated plugins/services, non-Windows native platforms and clean-machine WebView installation need separate device/account acceptance; they are not established by browser smoke.

### 本轮配置与会话竞态审核

- 模型目录后台刷新使用本地修订号保护，不再用保存前的迟到响应覆盖已经保存的新配置。UI 回归通过延迟真实目录响应复现旧问题，同时断言表单和连接标题（不能仅检查不受 props 更新的输入草稿）。
- 新连接创建与批量追加模型是两个独立后端写入。前端保留服务端归一化后的已创建 ID，追加失败后重试走更新接口，取消仍显示已经持久化的连接；不会把部分成功当成完全回滚。仅影响 Web 配置编辑，不改变 Runtime/TUI 协议或凭据存储。
- 配置写入进行中禁用对话框关闭入口，避免操作尚未完成时误以为取消已回滚；失败后可正常取消或重试。
- 未发送草稿按会话保留于当前页面内存，切换会话不丢失；整页重新加载不承诺保留。UI 回归分别测试会话切换和持久化后端偏好的 reload，避免混淆两种生命周期。
- 注入的模型保存与批量追加失败都有精确路径/状态断言；所有测试中暂扣的响应均有超时，不允许无界等待掩盖失败。

### 临时侧聊与单模型删除

- 辅助页面菜单新增“临时聊天”，每个主会话保留一个独立侧聊。首次发送时继承工作目录、模型、思考等级和权限，不进入普通历史列表。闲置 24 小时后清理；面板内显示“新开侧边聊天”，保留草稿并等待用户主动重开。关闭面板不停止执行，过期保护由 Runtime 判定。设计、生命周期和回滚见 [临时侧边聊天](architecture/side-chat.md)。
- 模型行提供删除按钮和确认，保留供应商连接与密钥。删除默认模型时优先选择同一连接的剩余可用模型，再选择其他已配置连接；没有可用模型时清空默认值。已有会话不自动换模型。
- `models.json` 的 Provider 新增可选 `excludedModels` 列表，避免内置目录或自动发现重新添加已删除项；显式添加（含旧配置接口）会取消排除。旧版本会忽略该字段，回退后被排除的内置或发现模型可能再次显示。普通模型定义与凭据格式保持兼容。
- Runtime 行为测试覆盖继承、幂等创建、24 小时过期、运行保护、重启与级联清理；模型测试覆盖默认回退、无模型状态重启、内置及发现模型删除和旧接口重新添加。UI 回归覆盖桌面/390px 侧聊、停止/恢复/审批、模拟到期重开和单模型删除确认。到期 UI 使用可控浏览器时钟及接口夹具，真实后端过期由服务测试验证。
