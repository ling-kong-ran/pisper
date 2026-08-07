# Pisper 优化机会

基于当前仓库结构、依赖引用、构建产物与核心大文件的审计结论。面向后续迭代排期，不是泛泛的代码风格建议。

相关背景：

- 架构与命令见根目录 [`AGENTS.md`](../AGENTS.md)
- 桌面 SEA 布局见 [`node-sea-webview.md`](./node-sea-webview.md)

## 总览

项目主架构清晰（`src` UI / `runtime` / Tauri / TUI），路由级懒加载已经落地。主要瓶颈集中在：

1. 超大模块难维护
2. 双栈 UI / 双栈 Markdown 带来的体积与复杂度
3. 未使用的 shadcn 组件与依赖
4. 工程化缺口（Runtime 无类型、CI 仅 release、格式化范围不一致）
5. TUI 的运行时能力选择与 workspace 继承缺少端到端保障

## 1. 高优先级：拆分“上帝模块”

| 文件 | 约行数 | 问题 |
| --- | ---: | --- |
| `runtime/runtime/agent-runtime.mjs` | ~3700 | 会话、provider、工具、记忆、多 agent、权限等全揉在一起 |
| `src/features/chat/ChatPage.tsx` | ~2000 | 页面状态、dock、流式同步、会话列表耦合 |
| `src/features/config/ConfigPage.tsx` | ~1700 | 设置面过大 |
| `src/features/chat/FocusSession.tsx` | ~1400 | 单会话交互与渲染过重 |
| `src/features/workflows/WorkflowsPage.tsx` | ~1400 | 列表 + 编排入口偏厚 |
| `runtime/http/api-handler.mjs` | ~660 | 巨型 `if (pathname…)` 路由链 |

### 建议

- **Runtime**：按域拆成可组合服务（session lifecycle / provider & models / streaming & transcript / tool activation），`AgentRuntimeService` 只做装配。
- **Chat**：`ChatPage` 只负责布局与数据编排；流式同步、dock 状态、session list 下沉到 hooks / stores。
- **API**：按资源拆 handler（`sessions`、`config`、`workflows`…），统一注册，而不是单文件顺序匹配。

**收益**：改一处少踩全局回归；测试可按域收敛（当前大量 case 堆在 `session-live`、`multi-agent` 等大测里）。

## 2. 高优先级：统一渲染与 UI 栈

### 2.1 双 Markdown 栈

同时存在两套路径：

| 路径 | 技术 | 主要使用方 |
| --- | --- | --- |
| 生产聊天 | `MarkdownMessage` → `react-markdown` + `rehype-highlight` | `ChatMessage`、`AgentRunActivity`、更新说明 |
| AI Elements | `streamdown` + `@streamdown/code\|cjk\|math`（偏 shiki） | `ai-elements/message.tsx`、`reasoning-content-body.tsx` |

构建产物中 `MarkdownMessage-*.js` 约 **328KB**。`message-shell` 注释也写明 Streamdown 尚未接到 Pisper 主协议。

`FocusSession` 中还有“流式用 plain `<pre>`，结束后再切完整 Markdown”的处理——说明主路径已在为双阶段渲染做补偿。

#### 建议（二选一，勿长期并存）

1. 聊天统一到 **Streamdown**（流式友好、与 ai-elements 一致），删除 `react-markdown` / `rehype-highlight` 路径；或
2. 将 ai-elements 的 Streamdown 明确为死代码路径，删除相关依赖与 chunk，只保留并优化 `MarkdownMessage`。

### 2.2 双 UI / 双 Toast

| 体系 | 位置 | 说明 |
| --- | --- | --- |
| 自研 | `src/components/ui.tsx` + 大块 `src/index.css` | Panel / Badge / Toast 等 |
| shadcn | `src/components/ui/*` + Tailwind | 新组件主栈 |

Toast 目前走自研 `Toast`；`sonner` + `next-themes` 基本只服务未接入业务的 `sonner.tsx`。

#### 建议

- 定一个主设计系统。新功能只走 shadcn / Tailwind。
- 旧 `ui.tsx` 按页面逐步替换，最终合并颜色、间距、圆角、阴影和交互状态 token，不要求一次性重写。
- 避免 CSS 变量皮肤与 Tailwind utility 两套 token 长期分叉；以项目设计 token 为唯一语义来源。

当前 CSS 产物约 **429KB**，很大概率来自双栈叠加与历史全局样式。

## 3. 中高优先级：依赖与产物瘦身

### 3.1 疑似未使用 / 仅被未使用组件引用

清理前请再跑一次全仓引用确认。

| 依赖 / 组件 | 审计现状 |
| --- | --- |
| `date-fns` | 源码 0 引用 |
| `recharts` + `ui/chart` | 仅组件自身，业务未用 |
| `react-day-picker` + `ui/calendar` | 同上 |
| `input-otp` | 同上 |
| `vaul` + `ui/drawer` | 同上 |
| `sonner` + `next-themes` | 几乎未接入真实 UI |

这些多半是 shadcn 脚手架带入。清掉可降低 `node_modules`、安装时间与 SEA runtime closure 体积。

### 3.2 前端 chunk 现状（`dist/assets`）

| Chunk | 约大小 | 说明 |
| --- | ---: | --- |
| `index-*.js` | 732KB | 壳层 + 公共依赖 |
| `ChatPage-*.js` | 455KB | 主路径，值得继续拆 |
| `MarkdownMessage-*.js` | 328KB | 与双 Markdown 相关 |
| `index-*.css` | 429KB | 样式双栈 |
| `WorkflowsPage-*.js` | 205KB | xyflow，可接受 |
| `react-bits-*.js` | 128KB | 装饰动效，可按页懒加载 |

路由 `lazy` 已在 `src/app/router.tsx` 落地。还可补充：

- Vite `manualChunks`：把 `react` / `react-dom`、`dockview-react`、`@xyflow/react`、`motion` 拆成稳定 vendor
- 装饰性 `react-bits` 仅在欢迎 / 记忆等页动态 import
- 评估 `axios`：本地 `127.0.0.1` API 用 `fetch` 通常足够（收益中等）

### 3.3 依赖分区与 Node 版本

- 几乎所有前端库在 `devDependencies`，runtime 只有少量包——对 Vite + SEA 能工作，但对新人与工具链不直观。
- 建议在文档中明确：**浏览器打包依赖** vs **sidecar 运行时依赖**；或后续拆 workspace。
- `package.json` 建议补 `engines.node`：README 写 20+，桌面打包文档与 CI 使用 24，当前不一致。

## 4. 中优先级：工程化与质量门禁

| 项 | 现状 | 建议 |
| --- | --- | --- |
| Typecheck | 只覆盖 `src/` + `vite.config.ts` | Runtime 渐进 TypeScript，或至少对 `shared/` 与关键边界加 JSDoc / `allowJs`+`checkJs` |
| Format | `.prettierignore` 忽略整个 `runtime/`、`scripts/`、`shared/` | 与前端统一格式化，避免 PR 风格分裂 |
| CI | 仅 tag release 跑 `check` / `test` | 增加 PR workflow（至少 `npm run check && npm test`） |
| Lint | oxlint 主要盯 `src` | Runtime 可加基础规则（unused、require-await 等） |
| 仓库垃圾 | `NUL`、`.tmp-*.log`、`.tmp-tauri-data/` 等 | 补 `.gitignore` 并清理，避免误提交 |

测试面本身不薄（`runtime/tests` 有大量 `.test.mjs`），缺的是提交前自动门禁与大模块可测边界。

## 5. 中优先级：运行时性能（聊天主路径）

已有不错细节：流式滚动分桶、未读态、结束后补 scroll。仍可挖掘：

1. **流式 Markdown**：结束瞬间从 plain text 切完整 AST 会造成布局跳动；统一流式渲染器可减少二次 layout。
2. **消息列表虚拟化**：长会话 + 多 session dock 时 DOM 线性膨胀；优先虚拟化 transcript。
3. **活动流 / tool 卡片**：`AgentRunActivity` 与 markdown 同屏时注意 memo 边界，避免 token 级父级重渲染。
4. **`agent-runtime` 热路径**：序列化 transcript、usage、live activity 的 helper 较多——拆文件后更容易做“只序列化脏消息”等针对性优化。

## 6. 中优先级：统一 Plan 术语并补齐 TUI 可视化

当前结构化执行计划存在术语分裂：runtime activity 和 Web 文案已经使用 **Plan**，但服务、工具、SSE、类型、持久化与部分 UI 仍叫 **Task List**。这里表达的是 Agent 的执行计划，不是独立任务实体；混用会让用户只看到 `UPDATE_TASK_LIST`，也会增加跨端协议和代码理解成本。

### 6.1 统一命名：Task List → Plan

将结构化计划子系统统一为 Plan：

| 当前命名 | 目标命名 |
| --- | --- |
| `TaskListService`、`task-list-service.mjs` | `PlanService`、`plan-service.mjs` |
| `TaskList` / `TaskItem` | `Plan` / `PlanItem` |
| `get_task_list` / `update_task_list` | `get_plan` / `update_plan` |
| `task_list_update` | `plan_update` |
| `taskList` / `task_list` | `plan` |
| `TaskBoard` | `PlanBoard` |
| `TASK_LIST_*` | `PLAN_*` |
| `pisper-task-lists.json` | `pisper-plans.json` |
| `{done}/{total} tasks` | `Plan · {done}/{total}` |

迁移要求：

1. **Plan 是唯一对外术语**：新工具 schema、SSE、前端/TUI 类型和用户文案只展示 Plan，不再暴露 Task List。
2. **保持升级兼容**：先让服务端双读旧字段、旧事件和旧存储文件，但只写新格式；旧工具名保留一个版本的兼容别名，且不作为首选工具暴露给模型。
3. **原子迁移持久化**：首次启动将 `pisper-task-lists.json` 迁移到 `pisper-plans.json`，迁移前备份，失败时继续读取旧文件，不能清空已有计划。
4. **按发布顺序收口**：服务端兼容层 → Web 与 TUI 消费新协议 → 清理旧标识；桌面、独立 TUI 与 sidecar 版本不一致时仍应可用。
5. **限定重命名范围**：只改“结构化执行计划”子系统。`followup_task`、subagent 的 delegated task、workflow/schedule task 等确实表示任务的概念继续保留 Task，不能机械全仓替换。

### 6.2 TUI Plan 可视化

服务端当前已经发送完整计划数据，Web 端也有计划面板。TUI 目前只反序列化条目状态，并在底部显示完成数；`App::apply_stream_event` 没有处理计划更新事件，因此用户主要看到工具活动，看不到具体计划内容和实时状态变化。

1. **补齐数据模型**：`PlanItem` 包含 `id`、`title`、`status`、`note`、`assignee`、`dependsOn`，并复用服务端既有状态语义。
2. **实时同步**：在 `meta`、`plan_update` 和 `done` 事件中更新当前 session 的 plan；空计划必须清掉旧内容，重连后从 session snapshot 恢复。
3. **紧凑计划面板**：在聊天流与 composer 之间显示 `Plan · 1/3`，逐项展示 pending / in progress / completed / blocked。默认突出当前步骤，空间不足时限制高度并折叠已完成项，不能挤占 composer。
4. **原位更新**：同一计划条目变化时更新现有行，不为每次 `update_plan` 追加重复内容；底部计数继续作为窄终端降级展示。
5. **可读性**：状态同时使用符号与颜色，标题优先于 note、负责人和依赖信息，保证无色终端仍能区分状态。

### 验收

- Agent 和用户只看到 `get_plan` / `update_plan` 与 Plan 文案；旧名称只存在于明确标注的兼容层和迁移测试。
- 收到 `plan_update` 后，下一个 redraw 能看到计划标题和状态，而不只是工具名。
- pending → in progress → completed / blocked 原位变化；清空计划后面板消失。
- session 恢复或 SSE 重连后计划一致，不显示上一轮的陈旧计划。
- `80×24` 与 `120×40` 下不遮挡消息、审批面板和 composer。
- 增加 Runtime、Web 和 Rust 测试，覆盖旧数据迁移、协议兼容、serde 字段、事件更新、清空语义及小终端渲染。

**收益**：统一 Agent、协议、代码和界面的概念模型，用户能直接看到当前执行步骤、剩余步骤和阻塞项。改动跨越 Runtime、Web 与 TUI，兼容迁移完成前风险中等。

## 7. 高优先级：修复 TUI thinking 等级与 workspace 继承

这两项不是体验微调，而是会改变 Agent 实际运行参数和文件访问边界的功能缺陷，应作为 **P0 可靠性问题**处理。

### 7.1 `/thinking` 没有可选等级或切换未生效

#### 已观察现象

- 在 TUI 输入 `/thinking` 后，选择器没有显示可选 level，用户无法确认当前思考等级。
- 即使执行切换，也缺少足够反馈证明 level 已写入当前 session，并用于下一次模型请求。
- 以 `gpt-5.6-sol` 为例，仓库元数据已声明 `off` / `xhigh` / `max` 映射，因此不能简单归因于模型不支持 thinking。

#### 当前链路与薄弱点

1. TUI 启动时通过 `GET /api/sessions/:id/thinking-level` 获取 `availableLevels`，但 `src-tui/src/main.rs` 使用 `.await.ok()` 静默吞掉请求错误。
2. 服务端候选来自 Pi session 的 `getAvailableThinkingLevels()`；TUI 的 `App` 又内置一份 `off` 到 `xhigh` 的通用列表，形成两个能力来源。
3. `set_thinking_options()` 会忽略空数组，无法区分“请求失败”“模型不支持”“能力解析回归”，界面也没有对应空态或错误态。
4. 切换后只更新本地状态栏；现有测试主要证明 picker 能发出 action 和 API 能转发字段，没有覆盖真实 provider/model 能力映射、session 状态持久化及下一次请求参数。

#### 优化要求

- **单一能力来源**：由服务端返回当前 provider/model 的规范化 thinking 能力；TUI 不硬编码并展示模型未声明支持的等级。
- **显式状态**：区分 loading、可选择、模型不支持和加载失败。空列表必须显示原因和重试入口，不能呈现一个无内容弹窗，也不能静默伪造默认候选。
- **模型联动**：`/model` 切换成功后立即刷新 thinking levels；当前 level 不再受支持时，由服务端选择合法回退值并在响应中明确返回。
- **端到端确认**：`PUT` 成功后重新读取或使用带版本的权威响应更新 session；状态栏显示 `Thinking · xhigh`，下一次请求必须携带映射后的 provider 参数。
- **诊断信息**：API 失败时保留可操作错误，Events/日志至少包含 session、provider/model、请求 level 和 available levels，不能记录凭证。

#### 验收

- `gpt-5.6-sol` 打开 `/thinking` 时显示与模型能力表一致的等级，选择后当前项标记立即更新。
- 切到 reasoning=false 或仅支持固定 reasoning 的模型时显示明确不可用原因，不显示空白列表。
- 模型切换、session 切换、TUI 重启后 level 与服务端一致；切换失败不会只改本地显示。
- 捕获下一次 provider 请求，验证 `off` / `xhigh` / `max` 被映射为正确参数。
- Rust 与 Runtime 集成测试覆盖成功、空能力、请求失败、动态 provider 模型和切换模型后的能力刷新。

### 7.2 启动 TUI 后 workspace 回退到用户目录

#### 已观察现象

从项目目录启动 `pisper` 后，会话 cwd 曾变成用户目录；本次 Agent 会话也实际以用户目录开始，手动切换后才恢复到项目根目录。预期是普通 `pisper` 始终以**启动命令时的当前目录**创建会话，除非用户显式传入 `--cwd` 或明确确认跨 workspace 切换。

#### 当前链路与薄弱点

- `launch_options()` 已使用 `std::env::current_dir()` 并 canonicalize，独立启动 sidecar 时也传入 `current_dir(workspace)` 和 `PISPER_WORKSPACE_DIR`，入口设计本身正确。
- 复用桌面 sidecar descriptor 或 `PISPER_TUI_URL` 时不会为共享 sidecar 设置启动 workspace，只依赖后续创建 session 的 `cwd` 请求；当前没有校验响应 cwd 与 launch workspace 一致。
- `/sessions` 可以切到其他 workspace 的历史会话，但界面没有醒目的目录边界提示或二次确认。
- `/new` 使用 `app.session.cwd`，而不是 TUI 启动时的 workspace；一旦当前会话 cwd 已回退或切到别处，错误目录会继续传播到新会话。
- session cwd 为空、无效或服务端回退为 `homedir()` 时，客户端缺少拒绝、修复和诊断机制。

#### 优化要求

- **保留 launch workspace**：在 TUI 生命周期中单独保存 canonicalized `launch_workspace`，不要用可随 session 切换的 `app.session.cwd` 代替。
- **创建不变量**：普通启动和 `/new` 默认使用 `launch_workspace`；只有显式 `--cwd`、专门的 workspace 切换动作或用户确认后才改变新会话目录。
- **响应校验**：创建 session 后校验服务端返回的 cwd；为空、无效或与请求不一致时中止进入会话，显示请求值与返回值，禁止静默回退到 home。
- **跨目录会话可见**：session picker 按 workspace 分组或默认只显示 launch workspace；打开其他目录的会话前明确展示目标路径并确认。
- **共享 sidecar 一致性**：独立 sidecar、桌面 descriptor 和 `PISPER_TUI_URL` 三种连接方式都必须通过 session API 传递并验证 workspace，不能依赖 sidecar 进程自己的 cwd。
- **启动可诊断**：`pisper doctor` 输出 launch cwd、连接方式和服务端 session cwd；状态区持续显示缩短后的路径，目录变化时给出一次明确通知。

#### 验收

- 分别从 PowerShell、CMD、bash/zsh 的项目子目录运行 `pisper`，首个 session cwd 与 shell 当前目录 canonicalize 后一致。
- `pisper --cwd <path>` 和 `pisper resume` 只进入指定 workspace；Windows 盘符大小写、符号链接和包含空格的路径比较正确。
- 覆盖新 sidecar、复用桌面 sidecar、`PISPER_TUI_URL` 三种启动路径。
- 切到其他 workspace 的历史 session 后执行 `/new`，默认仍回到 launch workspace；若产品决定继承当前 session，则必须显式提示并用测试固定该语义。
- 服务端返回空 cwd、home fallback、路径不存在或与请求不一致时，TUI 给出错误且不启动 Agent。

**收益**：确保 thinking 等级真实控制模型请求，并保证 Agent 的读写边界与用户启动 TUI 的目录一致。这两项都应先于视觉优化和结构性重构修复。

## 8. 桌面 / 打包（结构性，成本高）

- `release/sea` 与 `win-unpacked` 体积大：受 Pi 动态 Skills / MCP / native 约束，很难单文件化（见 [`node-sea-webview.md`](./node-sea-webview.md)）。
- 仍可做的：
  - 审计 SEA runtime 依赖裁剪脚本（dev-only、未用 locale、测试文件）
  - 大依赖保持动态 `import`（例如 `officeparser` 已是 lazy）
  - 清理历史 Electron 痕迹（如 `.prettierignore` 中的 `electron`）

## 9. 建议落地顺序

| 阶段 | 做什么 | 预期收益 | 风险 |
| --- | --- | --- | --- |
| **P0** | 修复 TUI `/thinking` 能力加载、切换透传与 workspace 启动继承 | 运行参数和文件边界可信 | 中（需 Rust/Runtime 端到端测试） |
| **P0** | 删未使用依赖 / 组件（`date-fns`、chart / calendar / otp / drawer / sonner 等，确认后） | 安装与包体立刻变轻 | 低 |
| **P0** | 选定唯一 Markdown 方案并删除另一套 | 聊天 chunk 与复杂度明显下降 | 中（需回归流式与代码块） |
| **P1** | 拆 `agent-runtime` + Chat 页面状态 | 开发速度与缺陷率 | 中高（需测试托底） |
| **P1** | PR CI + Prettier 覆盖 Runtime | 回归更早暴露 | 低 |
| **P1** | 统一 Task List → Plan 术语与协议，再补齐 TUI 紧凑计划面板 | 概念一致，计划内容与执行进度可见 | 中（需兼容迁移） |
| **P2** | 按页面迁移 legacy UI、统一 token 并压缩 CSS | 首屏与可维护性 | 中（视觉回归） |
| **P2** | 消息虚拟化 + vendor `manualChunks` | 长会话与缓存命中 | 中 |
| **P3** | Runtime 渐进类型化、API 路由表 | 长期质量 | 高投入 |

## 10. 暂不必优先动的点

- 路由级 code splitting（`createHashRouter` + `lazy`）
- Shiki web bundle + dedupe（`vite.config.ts` 已处理）
- `officeparser` 动态加载
- 流式滚动 / 贴底逻辑已有针对性处理
- release 流水线含 test / check / clippy
- 应用工具 `runtime/tools/app/` 一工具一模块的约定清晰

## 11. 结论

最值得做的不是继续堆功能，而是：

1. **纠偏**：先确保 TUI thinking 参数和 workspace 边界真实生效
2. **减负**：死依赖、双 Markdown、双 UI
3. **拆核**：`agent-runtime` / Chat / API
4. **补齐**：TUI Plan 等已有协议但缺少客户端呈现的能力
5. **把门**：PR CI、格式化与类型边界

推荐启动顺序：**P0 修复 TUI thinking/workspace 可靠性** → **P0 依赖审计清理 + Markdown 栈二选一** → **P1 核心模块拆分与 PR CI**。

## 12. 落地状态（2026-08-03）

本清单已按 [`optimization-implementation-plan.md`](./optimization-implementation-plan.md) 的 P00-P15 全部完成。对应结果如下：

| 原章节 | 状态 | 落地结果 |
| --- | --- | --- |
| 1. 大模块 | 完成 | `agent-runtime.mjs` 2315 行、`api-handler.mjs` 89 行、`ChatPage.tsx` 202 行、`FocusSession.tsx` 450 行、`ConfigPage.tsx` 111 行、`WorkflowsPage.tsx` 184 行 |
| 2. 渲染与 UI | 完成 | Streamdown 成为唯一生产 Markdown 栈；业务代码不再依赖 legacy `ui.tsx`，旧文件已删除 |
| 3. 依赖与产物 | 完成 | 直接依赖从 65 降至 54；移除未使用脚手架与前端 Axios；stable vendor chunk 和 gzip closure budget 已进入 build |
| 4. 工程门禁 | 完成 | PR CI 覆盖 Node quality/build/test 与 Rust fmt/check/test；runtime/scripts/shared 进入 Prettier；关键 JS 边界启用严格 `checkJs` |
| 5. 聊天性能 | 完成 | `@tanstack/react-virtual` 支持动态高度、streaming、prepend 锚点和有界 DOM；最终浏览器验收同时修复首次 scroll ref 挂载导致的零渲染 |
| 6. Plan | 完成 | 新协议只写 Plan，兼容读取旧 Task List；Web/TUI 均显示并原位更新 Plan，真实 Task 概念保持不变 |
| 7. TUI 可靠性 | 完成 | thinking 能力由服务端权威返回；launch workspace 独立保存、创建后校验，跨 workspace 切换显式可见 |
| 8. SEA | 完成 | 实际 production closure 裁剪 73.78%，最终 runtime 90.3 MiB，低于 120 MiB budget；关键动态依赖、许可证和当前平台 native 均保留 |
| 10. 保留能力 | 已回归 | route lazy、Shiki dynamic/dedupe、officeparser lazy、流式贴底、release checks 和 app-tool 单模块约定未回归 |

保留决策：

- 继续使用 `createHashRouter` 的路由级 lazy loading，不为改 URL 形态重写稳定路由。
- Shiki 语法、主题和 WASM 保持动态 chunk；Markdown 的完整动态 closure 以预算约束，不追求把高亮运行时重新塞回入口。
- `officeparser`、MCP clients、Playwright、pdfjs、skills runtime 和当前平台 native 是 SEA 的显式保留项。
- 浏览器 bundle 依赖继续位于根包 `devDependencies`，sidecar production closure 位于 `dependencies`；本轮不引入 workspace 或第二套包管理器。
- `checkJs` 先覆盖无扩散的 shared/runtime/API 边界，避免一次性破坏既有异步 facade 和 service 合约。
- 桌面仍是 Tauri 2 + Node SEA，不恢复 Electron 路径。

## 变更记录

| 日期 | 说明 |
| --- | --- |
| 2026-08-03 | P00-P15 全部完成，补充最终量化、保留决策与回归状态 |
| 2026-08-02 | 新增 TUI P0 可靠性问题：`/thinking` 无可选等级/切换未生效，以及启动 workspace 回退到用户目录 |
| 2026-08-02 | 新增 Plan 术语统一与 TUI 可视化优化：迁移 Task List 命名并展示步骤、状态与阻塞关系 |
| 2026-08-02 | 初版：基于仓库审计写入优化机会与落地顺序 |
