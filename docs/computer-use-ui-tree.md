# Computer Use UI 树文本化规范（序列化 + 索引设计）

本文档定义 Pisper computer use 栈中「UI 树文本化」的统一契约：序列化格式、ref 索引、
观察预算与增量 diff。它是决定模型「看得懂界面」程度的核心设计，所有平台采集实现
（macOS AX、Windows UIA、Linux AT-SPI，以及未来的 Pisper 原生采集器）都必须遵守，
不得各自发明格式。

## 架构分层

| 层 | 承载 | 职责 |
| --- | --- | --- |
| Agent 工具面 | 官方扩展 `@injaneity/pi-computer-use`（`find_roots` / `observe_ui` / `search_ui` / `expand_ui` / `inspect_ui` / `act_ui` / `read_text` / `wait_for` / 浏览器三件套） | UI 树采集、文本化、ref 解析、动作执行、act 前后 diff |
| 原生可视化 | `src-tauri/src/desktop_shell/computer_use.rs` | 窗口枚举、静态截图、实时镜像流（macOS SCStream，12fps 默认/60 上限） |
| 加固政策 | runtime 包装层（与 `tool-preview-images` 同层） | 帧预算、密码框 act 阻断、活动卡片预览 |

SEA 打包对 `@injaneity/pi-computer-use` 整包保留（`scripts/sea-runtime.mjs` 的
`preserveOfficialComputerUseSource`），官方 macOS/Linux/Windows 原生 helper 随 runtime
closure 分发，Windows 侧无需额外打包动作。

## 序列化契约（outline）

### 节点字段

每个 UI 节点序列化为下列字段（与官方 `OutlineNode` 对齐）：

- 身份：`ref`（公开引用）、`wireRef`（原生标识，见「ref 索引」）、`role`、`subrole`、
  `identifier`、`title`、`description`、`value`
- 能力矩阵：`actions[]` 与 `canPress` / `canFocus` / `canSetValue` / `canScroll` /
  `canIncrement` / `canDecrement` / `isTextInput`
- 几何与状态：`rect{x,y,w,h}`、`focused`、`offscreen`、`pictureOnly`（纯图片无文本）、
  `truncated`（子树被预算截断）、`scrollExtent{seen,total}`
- 文本证据：`text[]{string,confidence,rect?}`（OCR/Vision 结果，供模型对齐视觉与结构）
- 结构：`children[]`

### 文本渲染（给模型的 outline）

一行一节点，两空格缩进：

```
{ref} {displayName} {actions} [annotations] ▸ (folded summary)
```

- `displayName`：title/description/identifier/value 的最佳呈现
- `actions`：`{press,focus,...}`，仅列出该节点真实具备的动作
- 注解顺序固定：`offscreen`、`pictureOnly`、`truncated`、`scrollable seen/total`
- 折叠摘要：`▸ (后代数: 前 4 类角色统计) [scrollable seen/total]`——让模型不展开也
  能判断子树规模与构成

## ref 索引契约

- **公开 ref 形如 `@eN`**：模型可见、可引用；会话内单调分配，不复用给不同节点。
- **wireRef**：原生稳定标识（macOS 为 AX 元素令牌；Windows 为 UIA
  `RuntimeId`+`AutomationId`）。公开 ref 与 wireRef 双向映射由观察状态维护。
- **跨快照稳定（stabilizeRefs 三级策略）**：
  1. wireRef 精确匹配 → 沿用旧 ref；
  2. 结构键唯一匹配（`role|subrole|identifier|title|description` 逐层拼接 + 同级序号）→
     沿用旧 ref；
  3. 其余节点分配新 `@eN`。
  目标：界面小变动后模型手里的 ref 依然有效，减少重观察成本。
- **lookId/stateId 归属**：每次观察产生一个 state；`@e` ref 只在其所属 state 内可解析。
  `act_ui` 必须携带 `stateId`，跨 state 使用 ref 视为过期。
- **过期语义**：ref 解析优先「重解析」（Windows 按 RuntimeId/AutomationId 找回活元素，
  报告 `refound`），只有原生身份与结构键都失效才报 `stale_ref`。

## 观察预算与检索

- 默认渲染预算：`maxDepth=2`、`maxNodes=150`。
- 自动展开：focused 节点路径、sheet/dialog、被截断（`truncated`）子树。
- 预算耗尽时末行输出指引：`… render budget reached: N more nodes not shown; use
  search_ui or expand_ui(@eN)`——模型据此改用检索或定向展开，而不是盲猜。
- `search_ui`：文本分级匹配（exact > prefix > substring > fuzzy，fuzzy 用
  Damerau-Levenshtein）+ role 归一化匹配 + capability 过滤。
- `expand_ui(@eN)`：对指定子树做 scoped 深观察，结果嫁接回原 outline
  （`graftScopedOutline`），ref 体系保持一致。

## 增量 diff（AX diff）

- **act 前后**：官方 helper 在 act 时基线化根树，先用廉价的顶层窗口签名轮询等待早期
  稳定（`deltaSource: "win-poll"`），再做追赶快照，返回完整 before/after 差异；超时则
  返回全量快照路径（`"snapshot"`）。
- **字段级**：`changedFields` 忽略 ref/wireRef，rect 取整、confidence 两位小数后比较，
  避免噪声 diff。
- **结构级**：`OutlineChange`（出现/消失/移动）。
- Pisper 侧后续做「连续观察 diff」（observe→observe）时复用同一比较器，不另造格式。

## Pisper 集成决策

### 帧预算（M4，已落地）

原生镜像流与截图路径统一执行 **≤1280 宽、JPEG q70**：
`encode_jpeg` 质量 70；`maxWidth` clamp 上限 1280（默认 960）。q70 保证小字号文本
可读，1280 上限约束 IPC 载荷（配合 512KB 单帧硬顶）。

### 帧流通道：Tauri Channel 而非 SSE（刻意为之）

镜像帧走 Tauri event/Channel（`desktop_computer_use_start_stream` 的 `onFrame`），
**不走 runtime HTTP SSE**。原因：runtime 的 HTTP/SSE 输出必须同时满足浏览器
`JSON.parse` 与 TUI `serde_json` 的严格 UTF-8/JSON 约束（浏览器容忍 lone surrogate，
serde_json 直接断流）；帧流是桌面壳专属高频通道，走 Tauri Channel 天然绕开该约束，
也避免 base64 帧挤占会话事件流。后续新增高频/二进制桌面通道一律沿用此模式。

### 密码框控件级敏感关卡（已实现）

早期设计是「敏感 app 名单 + 会话级二次确认」，已废弃：「哪些 app 敏感」无法清晰
定义（名单武断且永远列不完），而密码框有系统级原生标记，边界清晰、可测试。

- **控件识别**（`runtime/services/computer-use-secure-refs.mjs`）：agent-runtime 在
  工具结果事件处喂入官方 outline 文本，扫描含 `AXSecureTextField`（macOS AX 原生
  role/subrole 标记）的行提取 `@eN` ref，登记进会话级注册表。不 fork 官方扩展：
  官方对密码框已拒绝读值（`secure_text_unreadable`）并置空序列化值，但允许写入——
  写入确认正是本关卡补上的最后一环。
- **act 阻断**（`SessionPermissionService.authorize`）：`act_ui` 的 `setText`/`typeText`/
  `keypress` 命中密码框 ref，或无 ref 且当前焦点推断为密码框（click 密码框后的
  焦点跟随输入）时，强制用户审批——auto 模式同样拦截（risk high）；full-access
  模式用户已声明完全信任，保持既有豁免语义。
- **审批载荷脱敏**：写入动作的 `text`/`keys` 可能含明文密码，审批事件与任何落盘
  产物只见 `•••`；审批决定**不进 5 分钟记忆缓存**（`skipRemember`，相同调用必须
  重新确认），避免密码哈希与密文进 `pisper-approvals.json`。
- **平台边界**：官方 Windows bridge 内部有 `isPassword`（UIA IsPassword），但归一化
  outline 节点不透出该标记（role 统一为 `edit`、值置空），控件级识别当前仅对
  macOS outline 生效；Windows 由安全输入状态上报兜底（见下），控件级透出的缺口
  可上游补 `isPassword` 注解后自然覆盖（扫描器只需追加标记词）。
- **测试**：`runtime/tests/computer-use-secure-refs.test.mjs`（11 用例：扫描/焦点
  推断/脱敏/审批集成/full-access 豁免）。

### 安全输入状态上报（已实现）

系统级安全输入激活时，合成键盘事件（agent 的 type/keypress）被操作系统拦截：

- macOS：Secure Event Input 锁（`IsSecureEventInputEnabled`，HIToolbox/Carbon，
  自 10.0 存在故直接强链，不同于 12.3+ 才有的 ScreenCaptureKit）。
- Windows：安全桌面（UAC/锁屏期间输入桌面为 Winlogon，`OpenInputDesktop` +
  `GetUserObjectInformationW(UOI_NAME)`，手写 user32 FFI 不新增 windows-sys feature）。
- 命令 `desktop_computer_use_secure_input_state` → 桥接 `computerUseSecureInputState`
  → 镜像面板（`ComputerUseLiveMirror`）在流式期间 2s 轮询，激活时显示警示横幅：
  agent 输入静默失效时用户能立刻知道原因，而不是归因为「自动化失灵」。

### M3 Windows 对等

- **Agent 工具面**：直接复用官方 `windows-bridge.exe`（随包分发）——UIA 树 +
  RuntimeId/AutomationId ref 重解析 + Pattern 优先接地（Invoke→Toggle→
  SelectionItem→ExpandCollapse→LegacyIAccessible，`grounding:"description"`）+
  raw input（SendInput 族）兜底 + `rootDelta`。Pisper 不自建 Windows UIA 采集。
- **原生可视化栈**：Pisper 镜像流/截图的 Windows 对等采用 **WGC（Windows.Graphics.
  Capture）**，运行时加载（`RoGetActivationFactory`/delay-load），避免旧 Windows 10
  版本强链接崩溃——与 macOS 侧「dlopen ScreenCaptureKit、主二进制零强链接」的约束
  完全对齐。官方 helper 的 GDI PrintWindow 捕获仅服务其自身 look image，不作为
  Pisper 可视化栈方案（PrintWindow 对 DirectX 内容与遮挡窗口有明显缺陷）。
  实现位于 `crates/computer-use-capture-win`（独立 crate：WGC 主路径 + GDI 轮询
  兜底），src-tauri 侧经 `computer_use.rs` 的 platform 分派层接入与 macOS 完全
  同构的监督线程/单槽邮箱/JPEG 编码管线（窗口编号统一 u64：macOS CGWindowID，
  Windows HWND）。WGC 帧为 GPU 纹理，经 staging 纹理 CopyResource+Map 回 CPU，
  尺寸变化才重建 staging；无边框（SetIsBorderRequired）/无光标
  （SetIsCursorCaptureEnabled）为 best-effort，旧系统忽略。
- **验收**：Windows 侧必须真机验收（R8 类比：源码检查 + 单测 + 实机运行分层报告）；
  当前开发机为 macOS，Windows 构建与运行验收需另行安排，交付时明确标注未验证项。

### 远期项

- 锁屏/私有桌面期间的**捕获**同样被系统阻断（注入侧已由安全输入状态上报覆盖）：
  镜像流可追加 `Error{code:"secure_input_active"}` 类事件细化提示；当前横幅轮询
  已能诚实报告。
- Windows 控件级密码框识别：待官方 outline 透出 `isPassword`（可上游 PR）。

## 现状与验收（2026-09）

- macOS：镜像流 SCStream 12fps 真机验证通过（Swift 原生置顶动画窗口目标，6s 73 帧
  ≈12.0fps，JPEG 编码正常；platform 分派重构后复验 PASS）；静止帧签名跳过与遮挡
  冻结（系统固有行为）已确认。
- 帧预算 q70/≤1280：已落地，cargo test 83 通过。
- Windows：官方 bridge 已随包分发（打包脚本确认保留）；Pisper 原生 WGC 镜像流已实现
  （crates/computer-use-capture-win + src-tauri platform 分派），在 macOS 主机上经
  `cargo check/clippy --target x86_64-pc-windows-gnu`（mingw-w64）与
  `--target x86_64-pc-windows-msvc`（捕获 crate）静态验证通过；**真机运行验收未做**
  （需 Windows 10 1903+ 实机：WGC 帧交付、GDI 兜底、降级链、长时间稳定性）。
- 敏感关卡：密码框控件级 act 阻断已落地（runtime 关卡 + 审批脱敏 + 不记忆，
  测试 11/11）；安全输入状态上报已落地（macOS Carbon / Windows 安全桌面 +
  镜像面板横幅）；Windows 控件级识别受限于官方 outline 未透出 isPassword，
  已记录为远期项。
