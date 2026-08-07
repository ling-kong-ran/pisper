# Pisper 优化实施计划

本计划将 [`optimization-opportunities.md`](./optimization-opportunities.md) 中的优化项拆成可独立审查、验证和提交的交付批次。

执行约束：

- 先冻结全部计划，再开始实现。
- 每个 plan 只产生一个聚焦 commit；验证失败不提交。
- 不使用 `git push`，不改写已有历史。
- 不提交用户现有的未跟踪 `AGENTS.md`。
- 并行工作使用独立 branch/worktree，完成后按依赖顺序 cherry-pick，避免共享工作区互相覆盖。
- 已经落地且文档明确列为“暂不必优先动”的能力只做回归验证，不为了产生 diff 重写。

## Definition of Done

每个 plan 必须同时满足：

1. 计划内行为与兼容要求完成，没有用 TODO 代替交付。
2. 新增或更新与风险匹配的 Runtime、Web 或 Rust 测试。
3. 运行计划列出的定向检查；涉及公共合约时再运行 `npm test` 和 `npm run check`。
4. 检查 `git diff --check` 和 staged diff，确认没有混入其他 plan 或用户改动。
5. 使用计划指定的 commit 主题提交，不 push。

最终验收统一运行：

```bash
npm run check
npm test
npm run build
npm run tui:check
npm run tui:test
npm run sidecar:sea:smoke
```

Tauri 平台打包只在当前机器依赖齐全时运行；不能运行的矩阵由 CI workflow 保证，并在最终记录中注明。

## 执行顺序

| 波次 | 可并行 plans | 进入条件 |
| --- | --- | --- |
| 0 | P00 | 无 |
| 1 | P01、P03、P05、P06、P11 | P00 完成 |
| 2 | P02、P04、P07 | 各自前置 plan 完成 |
| 3 | P08、P10 | P02/P04 完成；两者文件边界稳定后可并行 |
| 4 | P09 | P08 完成 |
| 5 | P12 | P04/P07/P09 完成 |
| 6 | P13 | P03/P12 完成 |
| 7 | P14 | 结构性改动全部完成，避免格式化冲突 |
| 8 | P15 | 所有实现 plan 完成 |

## 初始基线

基线采集于 `main` 的 `727cc63`：

| 检查/指标 | 结果 |
| --- | --- |
| `npm run check` | typecheck、lint、i18n 通过；format 仅因用户未跟踪的根目录 `AGENTS.md` 失败 |
| `npm test` | 368/369 通过；既有 `prompt-cache-optimizer-skill` 阈值失败，`reductionPercent=28.7` |
| `npm run build` | 通过，3182 modules，12.62s |
| `npm run tui:check` | 通过 |
| `npm run tui:test` | 42/42 通过 |
| 直接依赖 | 8 runtime + 57 dev = 65 |
| CSS | 438.50 kB，gzip 62.75 kB |
| app shell | 749.17 kB，gzip 237.16 kB |
| ChatPage chunk | 465.07 kB，gzip 112.38 kB |
| Markdown chunk | 335.14 kB，gzip 103.38 kB |
| react-bits chunk | 130.59 kB，gzip 42.83 kB |
| Workflows chunk | 208.93 kB，gzip 66.53 kB |

大文件基线：

| 文件 | 行数 |
| --- | ---: |
| `runtime/runtime/agent-runtime.mjs` | 3742 |
| `runtime/http/api-handler.mjs` | 663 |
| `src/features/chat/ChatPage.tsx` | 2063 |
| `src/features/chat/FocusSession.tsx` | 1404 |
| `src/features/config/ConfigPage.tsx` | 1748 |
| `src/features/workflows/WorkflowsPage.tsx` | 1431 |
| `src/components/ui.tsx` | 344 |
| `src/index.css` | 1396 |

P14 必须让质量命令不被未提交的本地 Agent 指令文件干扰，并修复或重新校准 prompt-cache 的既有稳定阈值；P15 的最终结果不得继续包含这两个失败。

## P00 冻结计划与基线

**目标**：提交原始优化清单和本实施计划，记录开始时的质量、体积与大文件基线。

**交付**：

- 保留 `docs/optimization-opportunities.md` 作为需求来源。
- 新增本实施计划，固定依赖图、验收规则和提交边界。
- 记录 `npm run check`、`npm test`、`npm run build`、TUI check/test 的基线结果。
- 记录关键 chunk 大小和六个大模块行数，后续 P15 做同口径对比。

**验收**：计划覆盖优化文档第 1-8 节和“暂不必优先动”回归项；工作区只 staged 两份 docs。

**Commit**：`docs: add repository optimization rollout plan`

## P01 TUI thinking 与 workspace 可靠性

**目标**：当前模型 thinking 能力和 TUI 启动目录成为服务端可验证的权威状态。

**交付**：

- thinking API 返回明确的 loading/supported/unsupported/error 语义和规范化 `availableLevels`。
- TUI 删除通用硬编码等级回退；请求失败和空能力有可读状态与重试，不使用 `.ok()` 静默吞错。
- `/model`、session 切换和 TUI 重连后刷新 thinking；PUT 使用服务端响应更新当前值。
- 保存 canonicalized `launch_workspace`；普通启动和 `/new` 默认使用它。
- 创建 session 后核对请求 cwd 与响应 cwd；拒绝空值、home fallback 和无说明的不一致。
- session picker 明确跨 workspace 边界；共享 desktop sidecar、`PISPER_TUI_URL` 和新 sidecar 使用相同 session cwd 合约。
- `pisper doctor` 输出 launch cwd、连接方式和服务端 cwd。

**测试**：Rust 单元/渲染测试；runtime API 测试；`gpt-5.6-sol` 的 `off/xhigh/max` provider 参数测试；Windows 路径规范化和三种 sidecar 连接测试。

**Commit**：`fix(tui): make thinking and workspace state authoritative`

## P02 Task List → Plan 兼容迁移和 TUI 面板

**目标**：结构化执行计划只有 Plan 一个对外概念，同时保证旧版本和旧数据升级安全。

**交付**：

- `PlanService`、`Plan`、`PlanItem`、`get_plan`、`update_plan`、`plan_update` 和 `PLAN_*` 成为规范命名。
- 首次启动原子迁移 `pisper-task-lists.json` 到 `pisper-plans.json`；迁移前备份，失败时继续读旧文件。
- 服务端双读旧字段/事件/文件，只写新格式；旧工具名保留一个版本的隐藏兼容别名。
- Web types、session state、activity、`PlanBoard` 和文案消费新协议；旧 event 只在兼容解析器中出现。
- TUI `PlanItem` 覆盖 id/title/status/note/assignee/dependsOn；处理 meta、plan_update、done 和清空语义。
- TUI 在 transcript 与 composer 之间渲染紧凑面板，80x24 下折叠完成项且不遮挡 composer/approval。
- followup/subagent/workflow/schedule 等真实 Task 概念不重命名。

**测试**：旧文件迁移与失败回退、工具别名、SSE 双读、新协议 Web reducer、Rust serde/事件/小终端 snapshot。

**Commit**：`refactor(plan): migrate structured task lists to plans`

## P03 依赖、脚手架组件和运行版本

**目标**：删除无业务引用的前端依赖和组件，明确 Node 与依赖分区。

**交付**：

- 再次确认并删除 `date-fns`、`recharts`、`react-day-picker`、`input-otp`、`vaul`、`sonner`、`next-themes`。
- 删除仅服务上述依赖的 `ui/chart`、`calendar`、`input-otp`、`drawer`、`sonner` 文件。
- 更新 lockfile，确认没有间接业务 import 被破坏。
- `package.json` 增加 `engines.node >=20`，打包文档继续明确 Node 24 目标。
- 在开发文档说明浏览器 bundle 依赖与 SEA runtime dependencies 的分区原则，不引入第二个 package manager/workspace。

**测试**：全仓引用扫描、`npm install --package-lock-only`、`npm run check`、`npm run build`。

**Commit**：`chore(deps): remove unused frontend scaffolding`

## P04 统一 Streamdown Markdown

**目标**：生产聊天、活动和更新说明统一使用一套支持流式增量的 Markdown 渲染器。

**交付**：

- 将 `MarkdownMessage` 改为 Streamdown 适配层，复用 cjk/code/math 插件实例。
- 保持链接安全、代码块复制、GFM、表格、引用、任务列表、CJK 与数学公式行为。
- 流式阶段不再 plain pre → 完整 AST 双阶段切换；同一消息组件增量渲染。
- `ChatMessage`、`AgentRunActivity`、reasoning 和更新说明使用同一适配层。
- 删除 `react-markdown`、`rehype-highlight` 和重复 Markdown CSS/代码路径。

**测试**：渲染 fixture、流式不完整 Markdown、危险链接/HTML、代码块和构建 chunk 对比；浏览器截图检查聊天与更新说明。

**Commit**：`refactor(markdown): standardize streaming message rendering`

## P05 ConfigPage 拆分与配置页 UI 迁移

**目标**：`ConfigPage` 只保留页面编排，设置域拥有独立组件和状态边界。

**交付**：

- 抽离 provider/model catalog、凭证、runtime/compaction、工具权限等领域组件和 hooks。
- 网络加载、保存、错误和 dirty state 不再集中在页面函数中。
- 迁移 Config 及既有子设置页的 Panel/Badge/Toggle/SectionTitle 到 shadcn primitives。
- 保持 i18n key、键盘操作、敏感字段遮罩和保存语义。

**测试**：typecheck、i18n、设置状态 reducer/hook 测试、浏览器桌面/移动截图；`ConfigPage.tsx` 目标低于 700 行。

**Commit**：`refactor(config): split settings domains and modernize controls`

## P06 WorkflowsPage 拆分与工作流 UI 迁移

**目标**：拆开工作流列表、编排、运行和预览职责。

**交付**：

- 抽离 workflow list/sidebar、editor canvas、node inspector、run controls 和 preview components。
- 图状态和 API mutation 下沉到 hooks；页面只做布局和路由选择。
- 迁移 Panel/SectionTitle/Segmented/Toggle/Badge/Metric 到 shadcn 与统一 token。
- 保持 xyflow 稳定尺寸、节点选择、保存、运行/停止和响应式布局。

**测试**：workflow graph 单元测试、typecheck、浏览器桌面/移动截图；`WorkflowsPage.tsx` 目标低于 700 行。

**Commit**：`refactor(workflows): split editor domains and modernize controls`

## P07 收敛 legacy UI、Toast 和 token

**目标**：业务代码不再依赖 `src/components/ui.tsx`，设计语义只有一套 token。

**交付**：

- 迁移 App overlays、chat、skills、plugins、memory、schedules、channels、assets、history 和 preview 页的 legacy primitives。
- 使用 shadcn Card/Badge/Switch/Tabs/Dialog/Toast-compatible primitives；命令按钮继续使用 lucide icon 与 tooltip。
- 将全局颜色、间距、圆角、阴影、focus/disabled 状态合并到 Tailwind 主题 token。
- 删除自研 `ui.tsx`、旧 Toast 和只服务 legacy primitives 的 CSS；避免卡片嵌套和页面 section 卡片化。

**测试**：全仓零 legacy import、i18n/check/build、主要路由桌面与移动截图、颜色/token 和文本溢出扫描。

**Commit**：`refactor(ui): retire legacy primitives and unify design tokens`

## P08 ChatPage 与 FocusSession 架构拆分

**目标**：聊天页面负责布局编排，状态同步和会话交互进入可测试模块。

**交付**：

- 抽离 session collection、dock lifecycle、SSE event reducer、stream synchronization、composer actions 和 scroll state。
- 将 transcript、composer、approval、empty state 和 dock panel 形成稳定组件边界。
- Zustand 只保存跨组件共享状态；瞬时 UI state 保持局部，避免全页 token 级重渲染。
- 保持 queue、stop、retry、approval、assets、Plan、多 Agent 和跨 cwd 会话行为。

**测试**：现有 chat tests 加 reducer/hook 测试；浏览器验证新建/切换/发送/停止/审批；`ChatPage.tsx` 目标低于 800 行，`FocusSession.tsx` 低于 700 行。

**Commit**：`refactor(chat): isolate session streaming and focus layout`

## P09 Transcript 虚拟化与活动流稳定渲染

**目标**：长会话 DOM 不再线性膨胀，流式更新不引发整个 transcript 重渲染。

**交付**：

- 使用成熟虚拟化库（优先 `@tanstack/react-virtual`）虚拟化消息 transcript，支持动态高度。
- 保持贴底、未读、向上阅读、代码块展开和流结束后测量。
- `ChatMessage`、`AgentRunActivity` 和 tool cards 建立稳定 key/memo 边界。
- Plan/approval/composer 不进入虚拟列表，不因测量跳动改变固定布局。

**测试**：1k 条动态高度消息、流式追加、历史滚动位置、窄屏和桌面 canvas 像素/截图检查。

**Commit**：`perf(chat): virtualize transcripts and contain streaming renders`

## P10 AgentRuntimeService 按域拆分

**目标**：runtime 主类只装配领域协作者，降低修改会话、模型、流和工具时的全局风险。

**交付**：

- 抽离 session lifecycle、provider/model preference、stream/transcript projection、tool activation 协作者。
- 明确依赖注入和返回类型，不允许新模块反向依赖完整 `AgentRuntimeService`。
- 保持现有 public runtime facade，HTTP 和 app tools 不感知内部拆分。
- 将 activity/usage/transcript 序列化改为按脏状态更新，避免无变化消息重复投影。
- 将相关大测试按域补充小型测试，不删除端到端回归。

**测试**：`npm test`、runtime resource/live/multi-agent/provider/model 定向测试；`agent-runtime.mjs` 目标显著低于 2500 行。

**Commit**：`refactor(runtime): compose agent domains behind the runtime facade`

## P11 API 资源路由表

**目标**：替换 `api-handler.mjs` 巨型顺序条件链，同时保持 HTTP/SSE 合约。

**交付**：

- 新增轻量 route registry，支持 method、静态路径、参数匹配和 handler context。
- 按 sessions/runtime、config/settings、workflows/schedules、memory/assets、integrations/desktop 拆 handler。
- 统一 JSON body、404、公开错误脱敏和 async error 边界。
- 路由冲突和注册顺序可检测；SSE 保持流式 response 的特殊生命周期。

**测试**：现有 runtime-api 全通过；增加每组路由匹配、参数 decode、404、异常脱敏与 SSE 测试；入口目标低于 150 行。

**Commit**：`refactor(api): register resource handlers through a route table`

## P12 Chunk、装饰模块与 HTTP client

**目标**：稳定公共 vendor 缓存，减少首页/聊天非必要代码并去掉可替代客户端依赖。

**交付**：

- Vite `manualChunks` 分离 react、dockview、xyflow、motion/animation 和 markdown/shiki，避免循环 chunk。
- 装饰性 react-bits 按实际页面 lazy import；移除 `main.tsx` 中不必要的全局装饰 CSS 入口。
- 将仅两处封装的 axios 迁移为有 timeout、abort、JSON/error normalize 的本地 fetch client，删除 axios。
- 记录 gzip 前 chunk budget，并让构建测试对明显回归失败。

**测试**：API client 单元测试、`npm run build`、路由懒加载检查、构建 manifest/chunk 大小对比。

**Commit**：`perf(web): stabilize vendor chunks and trim eager dependencies`

## P13 SEA runtime 裁剪和桌面痕迹

**目标**：对真实 runtime closure 做可证明安全的裁剪，并让产物组成可审计。

**交付**：

- 为 staged runtime 生成按 package/目录统计的 size manifest。
- 在现有 prune 基础上安全删除 dev/test/docs/source maps/types 和确认未用 locale；白名单保留动态 skills/MCP/native/officeparser 所需文件。
- 为动态 `officeparser` 和关键 native/package 加运行时存在性 smoke，避免过度裁剪。
- 删除 `.prettierignore` 等处遗留 Electron 路径，不重新引入 Electron packaging。
- 设置 SEA runtime 体积预算或相对基线告警。

**测试**：`npm run sidecar:sea`、`npm run sidecar:sea:smoke`、manifest 断言；当前平台可运行的 Tauri smoke。

**Commit**：`build(sea): audit runtime pruning and emit size manifests`

## P14 工程质量门禁与渐进类型化

**目标**：PR 在合并前覆盖 Web、Runtime 和 TUI 的基础质量，runtime/shared 获得一致格式和关键边界类型检查。

**交付**：

- 新增 pull_request workflow：Node 24 `npm ci`、`npm run check`、`npm test`、Rust fmt/check/test；使用 concurrency 取消旧 PR run。
- 缩小 `.prettierignore`，让 runtime/scripts/shared 进入格式化；机械格式化作为本 plan 的明确变化。
- oxlint 对 runtime/shared 增加 unused、require-await 等可执行基础规则并处理存量。
- 增加渐进 `checkJs` 配置和关键 shared/runtime/API JSDoc 边界，不要求一次改写全 Runtime 为 TypeScript。
- `.gitignore` 覆盖 `NUL` 大小写、临时日志和 Tauri 临时数据；清理可安全删除的已跟踪垃圾。
- 对本地 Agent 指令文件设置明确格式化策略，不修改或提交用户现有的未跟踪 `AGENTS.md`。
- 修复 prompt-cache measurement 的基线失败：优先恢复实际缓存收益；若依赖升级改变计量口径，则用有依据的稳定预算替换脆弱阈值。

**测试**：本地执行 workflow 的等价命令；`npm run check`、`npm test`、Rust check/test；Prettier 对目标目录零漂移。

**Commit**：`ci: enforce full repository quality gates on pull requests`

## P15 全栈验收与清单关闭

**目标**：用同口径数据证明优化完成，修复集成阶段发现的问题并回写文档状态。

**交付**：

- 运行 Definition of Done 的完整命令和可用桌面 smoke。
- 浏览器自动化覆盖 chat、config、workflows 和主要运营页面的桌面/移动视口；检查空白、重叠、溢出和 console error。
- 对比关键 chunk/CSS、SEA manifest、大文件行数、依赖数和长 transcript DOM 数。
- 验证既有 route lazy、Shiki dedupe、officeparser lazy、流式滚动、release checks 和 app-tool 单模块约定未回归。
- 在优化文档中逐项标记完成、保留决策和量化结果；记录无法在本机执行的平台矩阵。

**Commit**：`docs: close optimization rollout with verified results`

## Commit 序列

```text
01 docs: add repository optimization rollout plan
02 fix(tui): make thinking and workspace state authoritative
03 refactor(plan): migrate structured task lists to plans
04 chore(deps): remove unused frontend scaffolding
05 refactor(markdown): standardize streaming message rendering
06 refactor(config): split settings domains and modernize controls
07 refactor(workflows): split editor domains and modernize controls
08 refactor(ui): retire legacy primitives and unify design tokens
09 refactor(chat): isolate session streaming and focus layout
10 perf(chat): virtualize transcripts and contain streaming renders
11 refactor(runtime): compose agent domains behind the runtime facade
12 refactor(api): register resource handlers through a route table
13 perf(web): stabilize vendor chunks and trim eager dependencies
14 build(sea): audit runtime pruning and emit size manifests
15 ci: enforce full repository quality gates on pull requests
16 docs: close optimization rollout with verified results
```

## 最终验收（2026-08-03）

### Plan 状态

| Plan | 状态 | Commit / 结果 |
| --- | --- | --- |
| P00 | 完成 | `ccc75a4` |
| P01 | 完成 | `b42f05f` |
| P02 | 完成 | `d205838` |
| P03 | 完成 | `6ca6ea9` |
| P04 | 完成 | `26327de` |
| P05 | 完成 | `31e4b7a` |
| P06 | 完成 | `3c719bb` |
| P07 | 完成 | `59c580b` |
| P08 | 完成 | `8c1a873` |
| P09 | 完成 | `ae4568c` |
| P10 | 完成 | `0281af2` |
| P11 | 完成 | `a31e1d6` |
| P12 | 完成 | `1fb2be4` |
| P13 | 完成 | `aafe4e5` |
| P14 | 完成 | `c889fdf` |
| P15 | 完成 | 本提交；全栈验收中修复 virtualizer 首次取得空 scroll element 后不再订阅的问题 |

`c821458` 是 rollout 期间保留的独立 prompt-cache 测量稳定性修复，不属于新的 plan 交付单元；最终完整 Runtime suite 已包含该回归。

### 质量命令

| 命令 | 最终结果 |
| --- | --- |
| `npm run check` | 通过：TypeScript、增量 `checkJs`、oxlint、i18n、Prettier 全绿 |
| `npm test` | 430/430 通过 |
| `npm run build` | 通过：3258 modules，bundle budget 通过 |
| `cargo fmt --manifest-path src-tui/Cargo.toml -- --check` | 通过 |
| `npm run tui:check` | 通过 |
| `npm run tui:test` | 50/50 通过 |
| `npm run sidecar:sea` | 通过：Windows x64 SEA executable 与两个 Tauri sidecar target 已生成 |
| `npm run sidecar:sea:smoke` | 通过：production closure、Agent 激活、API 和正常退出均验证 |
| `git diff --check` | 通过 |

### Web 体积

直接 chunk 使用初始基线相同的 Vite 输出角色对比；`manualChunks` 后单个入口文件不再等同于冷启动总传输量，因此同时记录最终 closure budget，不用单文件下降冒充总下载下降。

| 指标 | 初始基线 | 最终结果 | 说明 |
| --- | ---: | ---: | --- |
| app entry chunk | 749.17 kB / 237.16 kB gzip | 133.08 kB / 40.22 kB gzip | React/UI/state 等进入稳定 vendor；entry 减少约 82%/83% |
| ChatPage chunk | 465.07 kB / 112.38 kB gzip | 152.94 kB / 45.31 kB gzip | 减少约 67%/60% |
| MarkdownMessage owner | 335.14 kB / 103.38 kB gzip | 2.97 kB / 1.47 kB gzip | 适配层变薄；Streamdown/Shiki 为独立动态 closure |
| react-bits welcome owner | 130.59 kB / 42.83 kB gzip | 4.42 kB / 1.84 kB gzip | 动效按页面加载；motion vendor 为 40.29 kB gzip |
| WorkflowsPage owner | 208.93 kB / 66.53 kB gzip | 41.40 kB / 12.69 kB gzip | xyflow vendor 为 55.64 kB gzip，可跨路由缓存 |
| global entry CSS | 438.50 kB / 62.75 kB gzip | 339.05 kB / 59.51 kB gzip | dockview/xyflow/React Bits CSS 改由路由所有 |
| entry static JS closure | 未建立同口径 manifest | 244.46 kB gzip | budget 260 kB |
| Markdown dynamic surface | 未建立同口径 manifest | 280.73 kB gzip | 包含 Streamdown、plugins、Shiki runtime/WASM；budget 320 kB |
| total split CSS | 未建立同口径 manifest | 71.17 kB gzip | budget 80 kB |

最终稳定 vendor gzip：React 88.55 kB、Dockview 75.89 kB、xyflow 55.64 kB、motion 40.29 kB、Markdown 126.92 kB、Markdown plugins 91.82 kB、Shiki runtime 60.30 kB；Shiki WASM 230.14 kB 保持动态归属。

### 文件与依赖

| 文件 / 指标 | 初始基线 | 最终结果 |
| --- | ---: | ---: |
| `runtime/runtime/agent-runtime.mjs` | 3742 行 | 2315 行；另有 498 行继承 facade，主文件 budget `<2500` |
| `runtime/http/api-handler.mjs` | 663 行 | 89 行 |
| `src/features/chat/ChatPage.tsx` | 2063 行 | 202 行 |
| `src/features/chat/FocusSession.tsx` | 1404 行 | 450 行 |
| `src/features/config/ConfigPage.tsx` | 1748 行 | 111 行 |
| `src/features/workflows/WorkflowsPage.tsx` | 1431 行 | 184 行 |
| `src/components/ui.tsx` | 344 行 | 已删除 |
| `src/index.css` | 1396 行 | 1394 行；收益体现在 legacy 规则替换和 route CSS ownership，不以行数包装效果 |
| 直接依赖 | 8 runtime + 57 dev = 65 | 8 runtime + 46 dev = 54，减少 11 个（16.9%） |

### SEA 与长会话

最终 `release/sea/runtime-size-manifest.json`：

- prune 前 `361,154,181 bytes / 28,492 files`。
- prune 后 `94,709,471 bytes / 10,604 files`，约 90.3 MiB。
- 减少 `266,444,710 bytes / 17,888 files`，体积下降 73.78%。
- 120 MiB runtime budget 通过；SEA executable `91,553,280 bytes`，约 87.3 MiB。
- 31 个关键 runtime 文件通过；仅保留当前 Windows x64 clipboard/pi-tui native，DOCX、MCP、Playwright、pdfjs、skills 和许可证 smoke 通过。

1000 条动态高度 transcript 的最终浏览器数据：

| 视口 | 已加载消息 | 最终 DOM 行 | 全程峰值 | 可见行 | prepend 锚点漂移 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1440×900 | 1000 | 11 | 19 | 5 | 0 px |
| 390×844 | 1000 | 9 | 16 | 3 | 0 px |

优化前 transcript 对每条消息线性创建 DOM；同一 1000 条输入意味着约 1000 个消息行。最终 DOM 始终低于 32 行。P15 首轮截图发现列表为 0 行：父级 host ref 在子级 layout effect 后才挂载，virtualizer 没有后续 render 去订阅 scroll element；callback ref 现在把真实元素变成显式状态并有源码回归测试。

### 浏览器矩阵

验收使用 gitignored `generated/browser/p15-agent` 作为独立 `PISPER_AGENT_DIR`，没有读取或修改真实 Pisper 会话。外部 GitHub compare 的 `/api/app-update` 在浏览器脚本中固定返回 `current`，避免网络状态污染 UI 验收；其他 API 均访问本轮 production preview。

- 视口：1440×900、390×844。
- 路由：chat、config、workflows、memory、MCP、skills、channels、schedules、assets、plugins、chat history。
- 两个视口的所有路由均非空、无 document 横向溢出。
- console error、page error、HTTP error 均为 0。
- 截图与机器可读结果位于 gitignored `generated/browser/p15-*.png` 和 `generated/browser/p15-browser-report.json`。

### 保留项回归

- `createHashRouter` 路由 lazy 和 route-owned CSS 保持有效，没有将 Dockview/xyflow/React Bits 样式重新放回全局入口。
- Shiki runtime、语言、主题和 WASM 保持动态 chunk，未产生循环 manual chunk。
- `officeparser` 继续只在文档附件路径动态 `import()`。
- streaming pinned/unread、用户滚动取消程序化贴底、prepend anchor 和结束后重测均由测试与浏览器数据覆盖。
- release workflow 继续运行 check/test/Rust clippy，并构建 Windows x64、macOS Intel/Apple Silicon、Linux x64 Tauri 产物。
- `runtime/tools/app/` 继续一工具一模块；Plan 兼容 wrapper 是明确的一个版本迁移层。

### 平台矩阵

| 平台 | 本机结果 | 后续门禁 |
| --- | --- | --- |
| Windows x64 Web / Node / Rust TUI / SEA | 已执行并通过 | PR CI 重跑 Node/Rust；release 重建 SEA/Tauri |
| Windows Tauri GUI CDP smoke | 未在本轮启动交互式 WebView2；该脚本会安装/切换桌面宠物并依赖 GUI/CDP，不在隔离会话验收中运行 | release workflow 的 Windows x64 `desktop:webview:build` |
| macOS Intel / Apple Silicon | 本机不可执行 | release matrix |
| Linux x64 | 本机不可执行 | release matrix |

所有 plan 均已完成；未 push、未改写历史，用户未跟踪的 `AGENTS.md`、`docs/agent-sandbox-design.md`、`docs/promotion-zh-CN.md` 未读取、未修改、未提交。
