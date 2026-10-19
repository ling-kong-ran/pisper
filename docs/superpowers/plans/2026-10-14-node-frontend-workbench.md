# Node 基线前端工作台实施计划

> 2026-10-16 恢复整合说明：本文件记录原工作台设计/验收背景。其中自建项目文件浏览器、目录枚举接口和相应验收已被撤回，恢复时不再带入；当前边界以[系统文件选择](../../architecture/system-file-selection.md)为准。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans 按任务实施；如用户改选子代理执行，再使用 superpowers:subagent-driven-development。步骤用复选框记录，不以计划编写完成代替功能完成。

**Goal:** 在 `release` 的 Node Runtime 基线上完整实现已确认的前端工作台、样式规范及四类页面模板。

**Architecture:** 应用壳组织常驻功能栏与可收起项目侧栏；聊天领域持有按任务隔离的视图状态，Dockview 只负责分屏与焦点；内容标签通过现有领域 API 和终端公共接口消费真实资源。公共布局组件仅接收插槽和参数。保留 Node Runtime，不引入新的 Rust 后端或业务协议。

**Tech Stack:** Node.js 24、React、TypeScript、Tailwind 4、shadcn、Zustand、React Query、Dockview、现有 Tauri 原生终端桥接、Node test runner 与 Playwright。

**Spec:** [前端工作台设计方案](../specs/2026-10-14-frontend-workbench-design.md)。其中“仅写文档”的历史阶段说明已由用户本次实施指令取代，产品范围与验收规则不变。

## Global Constraints

- 按用户要求直接在当前 `release` 工作区实施，不创建或切换到新的开发分支。
- 产品基线为 `v0.5.81`，恢复提交为 `1f1c4000`。不得按 Rust 分支的旧路径整文件覆盖当前源码。
- 当前未提交的 Release 更新说明改动属于上一任务，必须保留；本任务不改写这些文件的已有内容，不创建发布版本或标签。
- Node 验证使用 `/opt/homebrew/opt/node@24/bin` 对应的 Node 24；锁文件不升级，不新建包管理器。
- 保留 `build.target: 'safari16'` 和构建兼容性检查，不新增全局业务 CSS 类，不扩大基础组件到 Feature 的依赖。
- 用户可见文案登记 `zh-CN`、`en-US` 字面量键；共享公共接口保持兼容或提供迁移。
- 草稿、临时网页地址、来源正文、终端输出不进入通用页面偏好；需要持久化的 UI 偏好使用实际 Node 路径 `src/lib/page-state-storage.ts` 与白名单。
- 内容标签上限沿用 12；原型两分屏不是产品上限。功能栏在手机也保留，内容区按局部可用宽度切换。
- 只使用已有资源能力。Node 基线未发现通用进程枚举 HTTP 端点；先展示真实的应用自有终端进程与日志，不冒充全部系统／代理进程。不可用能力有明确状态。
- 不读真实密钥或启动用户会话。浏览器测试使用临时目录、隔离 Runtime 与受控模型／桥接。

## Review Focus

1. A 任务请求尚未返回就切到 B：返回值只能更新 A。任务 4、6 的乱序响应测试覆盖。
2. 关闭标签或分屏时终端仍在运行：连接、目录及后台输出保留。任务 5 的真实桥接事件 fixture 覆盖。
3. 跨项目同名文件／同名任务、失效会话标识：按稳定身份定位，不按标题匹配。任务 3、4、6 覆盖。
4. 320px、长中文／英文、用户字号和软键盘：输入、关闭入口与焦点可达。任务 7、8、9 覆盖。
5. 旧 Dock 布局包含任务标签和网页面板，或快照损坏：兼容恢复或安全回退，不能静默覆盖无法识别的状态。任务 4、9 覆盖。

## 当前契约与文件映射

| 设计中的职责    | 当前 Node 源码与契约                                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| 应用壳、导航    | `src/App.tsx`、`src/app/navigation.ts`、`src/app/routes.ts`、`src/app/route-elements.tsx`                                             |
| 侧栏            | `src/components/layout/AppSidebar.tsx`、`SidebarRecentSessions.tsx`、`WorkbenchSidebarToggle.tsx`                                     |
| 会话编排与 Dock | `src/features/chat/ChatPage.tsx`、`ChatDock.tsx`、`use-chat-dock.ts`、`dock-layout.ts`、`chat-dock-context.ts`                        |
| 内容状态与布局  | `src/features/chat/session-context-pages.ts`、`session-context-pages-store.ts`、`SessionContextPanel.tsx`、`SessionContextLayout.tsx` |
| 草稿和阅读      | `src/features/chat/composer-drafts.ts`、`FocusSession.tsx`、`FocusTranscript.tsx`                                                     |
| 文件与差异      | `src/features/chat/SessionFilesPane.tsx`、`GitChangesControl.tsx`、`FileChangesControl.tsx`、`GitDiffViewer.tsx`、`chat-api.ts`       |
| 终端            | `src/features/terminal/TerminalPanel.tsx`、`terminal-session-scope.ts`、`terminal-state.ts`、`src/types/update.ts`                    |
| 主题            | `src/index.css`、`src/lib/custom-accent.ts`、`src/app/ui-preferences.ts`、`src/stores/ui-store.ts`                                    |
| 通用组合        | `src/components/ui/app-primitives.tsx`、`src/components/layout/PageHeader.tsx`                                                        |
| 校验入口        | `runtime/tests/*.test.mjs`、`scripts/smoke-pisper-ui.mjs`、`scripts/smoke-workflows-ui.mjs`、`.github/workflows/ci.yml`               |

这批修改不机械搬迁整个 Feature。新增模块按真实所有权放在当前同域中；原有基础组件里的页面祖先选择器由显式布局参数替代。

Node 基线没有 Rust 分支的 `test:chat-dock`、`test:sidebar-projects`、`test:mobile-shell`、`test:ui-ux` 脚本入口。不得引用为既有检查；任务 9 将补齐等价、实际可运行的前端验收入口。

## Task 1 样式缺陷和颜色契约

**Files:** 修改 `src/index.css`、`src/lib/custom-accent.ts`；扩展 `runtime/tests/custom-accent.test.mjs`；新增 `runtime/tests/theme-contract.test.mjs`。

**Interfaces:** 保留 `customAccentStyleRules(hex: string): string`、`normalizeHexColor(input: string): string | null`；补齐已被调用的 `bg-surface-subtle`、`bg-surface-hover`、`text-brand` 映射。颜色角色统一在主题变量中定义，不向页面传裸 HEX。

- [x] 用实际 Tailwind 编译入口断言三种工具类生成，并用实际 CSS 计算色验证各预设文字对比；先确认缺陷测试失败。
- [x] 修复实心强调前景、自定义极亮／极暗焦点，保留旧 WebView 透明度降级；不在这一任务重排页面。
- [x] 为固定字号与 `small` 覆盖建立代表控件测试，在调用方改为排版角色后才收敛全局规则。
- [x] 运行 `node --import tsx --test runtime/tests/custom-accent.test.mjs runtime/tests/theme-contract.test.mjs`，再检查构建中的 CSS 输出。

## Task 2 中性色彩体系和公共布局组件

**Files:** 修改主题、`src/components/ui/app-primitives.tsx`、`src/components/ui/button.tsx`、`src/features/config/InterfaceSettings.tsx`、对应翻译；新增 `src/components/app/PageFrame.tsx`、`PageToolbar.tsx`、`ListDetailLayout.tsx`、`PageState.tsx`；新增 `runtime/tests/page-layout-contract.test.mjs`。

**Interfaces:**

- `PageFrame({ variant, children, className })`：`variant` 为 `workbench | collection | settings | settings-wide`，只负责宽度、外边距和滚动。
- `PageToolbar({ children, actions })`：搜索过滤区域与操作插槽，不接收业务实体。
- `ListDetailLayout({ list, detail, inspector, selected, onBack })`：独立滚动区域与窄屏列表／详情切换。
- `PageState({ kind, title, description, action })`：`kind` 为 `loading | empty | error`，不改变外层页面骨架。

- [x] 建立公共组件不依赖 Feature、同类状态容器宽度不变、窄屏返回保留选择的行为测试。
- [x] 固定中性 `primary`、中性 hover／选择表面，建立独立的交互、焦点、信息与状态色；保留用户预设值，仅收窄颜色作用范围并同步设置说明。
- [x] 去掉共享组件中的 `.memory-layout`、`.skill-scope-head` 等业务父级条件；调用页显式提供自身布局。
- [x] 通过代表页核对常规设置宽度 960px、宽版 1040px 的候选值；只有在两种语言与字号设置下可用才固定为模板值。
- [x] 运行主题、公共布局测试、typecheck、lint 和 i18n 检查。涉及共享默认外观的改变同时检查现有空态与模态窗。

## Task 3 三层应用壳和项目导航

**Files:** 修改 `src/App.tsx`、`src/app/navigation.ts`、`src/components/layout/AppSidebar.tsx`、`WorkbenchSidebarToggle.tsx`、`MobileNavigation.tsx`；新增 `src/components/layout/ActivityRail.tsx`、`WorkbenchHeader.tsx`；将任务列表业务从 `SidebarRecentSessions.tsx` 迁到 `src/features/chat/ProjectTaskList.tsx`，通过新建 `src/features/chat/public-navigation.ts` 暴露；扩展对应翻译及导航测试。

**Interfaces:** `ActivityRail({ groups, activeId, onSelect, footer })` 与 `WorkbenchHeader({ title, sidebarOpen, onToggleSidebar, actions })` 均只消费展示数据。`ProjectTaskList` 提供既有项目、任务与确认对话行为，由应用壳通过公开入口组装。

- [x] 先写导航断言：隐藏侧栏后 rail 与顶部开关仍可达；不同项目同名任务按 ID 定位；资源菜单以当前任务为目标。
- [x] 建立工作台／项目、资源、自动化、扩展、底部设置分组；已有页面和配置入口继续可达，能力过滤保留。
- [x] 项目内任务与无项目任务分组不重复；迁移搜索、置顶、归档、改名和项目选择回调，不重写相关 API。
- [x] 桌面侧栏宽度和展开走现有持久化，手机覆盖层开关仅存内存；新增偏好键才登记白名单。
- [x] 在桌面和 320／390px 校验选择、关闭、键盘焦点与长标题。覆盖 L01、L02、M01。

## Task 4 任务工作区状态与 Dock 装配

**Files:** 修改 `src/features/chat/session-context-pages.ts`、`session-context-pages-store.ts`、`composer-drafts.ts`、`dock-layout.ts`、`use-chat-dock.ts`、`ChatDock.tsx`、`ChatPage.tsx`、`FocusSession.tsx`、`FocusTranscript.tsx`；新增 `TaskWorkspace.tsx`、`TaskWorkspaceHeader.tsx`；扩展现有 context、draft、dock 测试。

**Interfaces:**

- 在当前 `ContextPage` 中增加资源身份，而不是以标题区分标签；类型覆盖 `new | changes | files | terminal | processes | sources | browser | plan | side-chat`。
- 按任务维护 `pages`、`activeId`、`contentOpen`、`summaryOpen`、对话阅读位置及标签内视图状态；宽度偏好继续由现有布局 Store 持有。
- `TaskWorkspace({ sessionId, panelId, active, children, onSplit, onClose })` 为独立显示边界，内容和摘要在任务内部装配。
- 扩展 `ContextPageAction` 的打开／复用、空白标签转换、关闭和资源 UI 更新动作；超出 12 页返回明确结果给通知层。

- [x] 先扩展状态测试：A/B 独立标签；最后一页关闭后零内容页；同身份复用；空白入口转换；超过 12 页拒绝；草稿超过旧缓存界限仍不静默丢失。
- [x] 保留会话流与查询的现有所有者，移除聊天页外层唯一内容面板，把真实任务工作区挂到 Dock 分屏内。
- [x] 普通选任务只替换焦点分屏；已显示任务直接聚焦；显式上下左右分屏保留原任务。空间不足只提示，不替换其他任务。
- [x] 恢复旧布局时解析任务与网页面板，兼容有效内容；失败时保留旧快照并给出安全回退，不覆盖损坏布局。
- [x] 用行为测试验证快速切换、延迟恢复、关闭运行任务视图不 abort；回到任务恢复草稿、选中标签与阅读位置。覆盖 W01、W02、T01–T03、S01–S04、C02。

## Task 5 终端生命周期与进程视图

**Files:** 修改 `src/features/terminal/TerminalPanel.tsx`、`terminal-session-scope.ts`；新增 `terminal-registry.ts`、`public.ts`、`TaskTerminalView.tsx`、`TaskProcessesPane.tsx`；新增 `runtime/tests/terminal-registry.test.mjs`，扩展 `terminal-session-scope.test.mjs`。

**Interfaces:** `createTerminalRegistry(transport)` 显式创建实例；提供 `create(sessionId, cwd, profileId)`、`subscribe(listener)`、`snapshot(sessionId)`、`write(id, data)`、`resize(id, cols, rows)`、`stop(id)`、`dispose()`。实例由应用生命周期持有，导入模块不启动连接。标签只持有终端 ID。

- [x] 在受控真实接口形状下测试创建 A 终端、切到 B、移除 A 视图：不得调用 close；只有 stop 或宿主退出触发资源释放。
- [x] 从视图迁出连接和运行实例所有权，保留创建时的会话、目录与桥接绑定。已删除会话的终端标记原归属失效，不重绑到另一个项目。
- [x] 进程面板展示实际应用自有终端、状态筛选和独立日志；未知状态为中性状态。缺少原生桥接时明确不可用，不新增未经设计的后端端点。
- [x] 日志支持自动跟随开关；用户向上滚动不被新输出拉回底部。资源退出保留可解释的标签。
- [x] 运行终端定向测试，真实 PTY 行为只使用隔离工作目录。覆盖 T04、P01、C01。

## Task 6 真实资源标签与摘要

**Files:** 修改 `src/features/chat/SessionContextPanel.tsx`、`SessionFilesPane.tsx`、`GitDiffViewer.tsx`、`chat-api.ts`；新增 `TaskSummary.tsx`、`TaskSourcesPane.tsx`、`task-sources.ts`、`TaskNewTab.tsx`；扩展 context、文件 API 和来源测试。

**Interfaces:** `collectTaskSources(messages: readonly ChatMessage[]): TaskSource[]` 以现有消息附件生成只读来源投影；`TaskSource` 使用材料 ID 或规范化引用确定身份。`TaskSummary` 消费任务的资源摘要与回调，不自行持久化业务实体。

- [x] 先覆盖同名来源、未知附件形状、空数据与失效引用；不得用未验证字段拼接可执行 URL。
- [x] 新标签入口接通文件、差异、终端、来源、网页、计划与侧聊；显示与实际 capability 和桥接能力一致。
- [x] 变更一行一文件，默认全收起，可多项展开及全部收放；单文件差异按需获取，状态属于标签；保留既有审批／撤销流程。
- [x] 项目 Git 变更与任务快照分别标注范围，不把共享目录变更说成任务独占；加载错误不显示为零变更。
- [x] 来源搜索、类型筛选、详情和附件预览复用既有 API；添加引用沿用现有附件操作，不保留原型的“添加示例”。
- [x] 摘要使用桌面非模态浮窗与手机抽屉；点击条目定位已有标签和具体资源。确认迟到结果不会更新另一个任务。覆盖 F01、D01、R01、C01。

## Task 7 移动端与局部响应式

**Files:** 修改 `SessionContextLayout.tsx`、`session-context-layout.ts`、应用壳、`MobileNavigation.tsx`、`MobileViewportStabilizer.tsx`；新增 `src/features/chat/workspace-presentation.ts`；新增 `runtime/tests/workspace-presentation.test.mjs`。

**Interfaces:** `resolveWorkspacePresentation({ width, mobile, contentOpen }): 'conversation' | 'split' | 'content'`，参考 660px 的局部宽度；返回栈由当前弹层、覆盖侧栏、内容视图的所有者逐层消费，不由浏览器历史承担临时视图状态。

- [x] 先测试 320／390px、660px 边界、平板切换与多个任务：宽度变化不修改任务集合和持久化分屏意图。
- [x] 手机保留功能栏，侧栏覆盖主内容，摘要底部抽屉限制可用高度并处理安全区域。
- [x] Escape／系统返回一次只关闭最上层；抽屉背景不可操作，关闭恢复焦点；非模态桌面摘要不锁定主内容。
- [x] 验证软键盘、横向代码滚动、拖动分隔条、触摸关闭与字体放大。覆盖 F02、M01、M02、A01。

## Task 8 四类页面模板迁移

**Files:** 修改 `src/features/decisions/DecisionsPage.tsx`、`src/features/config/ConfigPage.tsx` 及设置组件、`src/features/schedules/SchedulesPage.tsx`、`src/features/mcp/McpPage.tsx`、`src/features/skills/SkillsPage.tsx`、`src/features/channels/ChannelsPage.tsx`、`src/features/memory/MemoryPage.tsx`、`src/features/assets/AssetsPage.tsx`、`src/features/plugins/PluginsPage.tsx`、`src/features/workflows/WorkflowsPage.tsx`、`src/features/custom-ui/CustomUiPage.tsx`、`src/features/custom-ui/CustomUiToolPage.tsx`；调用方、路由装配及相关既有 UI 测试同步。

- [x] 3A 先迁移决策模型与外观设置，确认表单宽度、校验和加载骨架，再覆盖其他设置；复杂模型配置用宽版。
- [x] 3B 以计划任务验证列表详情与手机返回，再迁移 MCP、技能、渠道、记忆列表；保留各域的查询、保存和实时状态。
- [x] 3C 迁移资源库、插件和工作流集合页，统一过滤工具栏和空态；不改变分页、安装、预览和导入行为。
- [x] 3D 工作流编辑器与画布按实际容器尺寸布局；聊天工作台以任务 4–7 的实际交互为准。
- [x] 自定义界面目录采用集合模板，工具宿主采用满幅工作台；通过现有宿主尺寸适配游戏资源等内嵌工具，不改写第三方内容或放宽 CSP。
- [x] 每个页面在迁移时移除自己的重复外边距和重复整页滚动，不先全局删除未迁移页面仍依赖的规则。
- [x] 对设置保存失败、无选中项、长标题、空集合、三栏检查器和跨路由返回建立回归。覆盖 PT01–PT04。

## Task 9 验收入口与残留收口

**Files:** 新增 `scripts/smoke-frontend-workbench.mjs`、`scripts/fixtures/frontend-workbench.tsx`、`runtime/tests/frontend-workbench-contract.test.mjs`；修改 `scripts/smoke-pisper-ui.mjs`、`package.json`、`.github/workflows/ci.yml`；同步 `docs/development-workflow.md` 与架构文档中的适用路径。

- [x] 新增 `npm run test:workbench`，使用真实 React、Dockview 与生产 CSS；仅在 HTTP、模型和原生桥接边界提供受控 fixture，不把工作区或标签组件替换成假组件。
- [x] CI 在前端构建与浏览器准备后执行该入口；Node 纯模型和协议测试纳入现有 `npm test`，不建立未被 CI 发现的测试目录。
- [x] 逐项执行 L01–L02、W01–W02、T01–T04、S01–S04、F01–F02、D01、P01、R01、M01–M02、A01、C01–C02、ST01–ST04、PT01–PT04；记录对应测试名、截图和实际平台。
- [x] 核实旧别名和业务祖先选择器已无调用方后再清理。保留仍需支持的第三方适配、语音、终端与 Markdown 特殊样式。
- [x] 最终执行 `npm run check`、`npm test`、`npm run build`、`npm run test:workbench`、`npm run test:ui`；修改后的守卫保留原安全／协议保护目标。
- [x] 桌面 WebView、Android 和 iOS 分别记录浏览器模拟、构建、设备运行；未具备环境的项列为未验证，不能宣布完整平台验收通过。
- [x] 只读独立评审集成差异，修复重要问题后按完整规格审计；不以某一批通过代替整体完成。

## 覆盖与交付记录

| 规格范围                              | 实施任务 | 完成证据                                 |
| ------------------------------------- | -------- | ---------------------------------------- |
| 第 3 节三层布局                       | 3、7     | 导航与手机交互检查                       |
| 第 4–7 节任务、标签、摘要、分屏       | 4、6、7  | 状态模型与真实组件测试                   |
| 第 8–10 节变更、进程、来源            | 5、6     | 真实数据适配与错误、生命周期测试         |
| 第 11–12、14 节移动端、状态、可访问性 | 3–7、9   | 焦点、恢复、软键盘与多平台记录           |
| 第 13 节样式与模板                    | 1、2、8  | 编译、颜色、模板和视觉检查               |
| 第 15–16 节职责、迁移和分批范围       | 全部任务 | 接口、调用方与迁移差异审查               |
| 第 17 节完整验收                      | 9        | 每个编号均有直接证据，未通过不得记为完成 |

代码执行采用当前会话逐批集成的方式。开始产品改动前，先确认这份针对 Node 基线的具体计划；执行后在本文件勾选真实完成项，并单独保存完整验收记录。


本次工作区验收与平台边界见[完整验收记录](../reports/2026-10-15-frontend-workbench-verification.md)。用户要求暂不提交或发布，所有实现保留为工作区改动。
