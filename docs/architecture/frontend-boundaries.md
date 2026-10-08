# 客户端请求与 Feature 边界治理

- 状态：已采纳
- 日期：2026-09-22
- 影响范围：Web、桌面端、Android/iOS 共用的 React 客户端；无 Runtime/TUI 线协议或持久化格式迁移。

## 问题与决策

统一 HTTP 客户端曾把 JSON 解析失败降级为成功文本，SSE 又单独解析 HTTP 错误。现在由 `src/lib/http-response.ts` 统一解析，`api-error.ts` 定义错误类型，`http.ts` 保留传输、超时和取消的所有权，并继续导出 `ApiError` 兼容旧调用方。无效 JSON 返回 `kind: protocol`、`data.code: INVALID_RESPONSE`，不附带响应正文；明确需要文本的调用使用 `requestText`。204 和空成功响应仍兼容返回 undefined。新领域 API 通过 `parse` 从 unknown 校验业务结构；旧泛型调用尚未全部迁移，不能将此变更理解为所有 API 都已完成字段校验。

MCP 页面过去位于 workflows，直接拼接端点、保存远程快照和启动定时器，读取请求可能覆盖操作结果。现在 MCP API 校验并投影页面所需字段，React Query 是远程快照的唯一所有者，页面仅持有选中项。读取消费 AbortSignal，最后一个观察者离开会取消请求。写操作串行、无自动重试，先取消旧查询，成功后更新缓存并失效；失败保留最后的快照并允许重新读取。进入页面及轮询只读取状态，不再强制重连全部 MCP；测试连接、启用及新增仍由显式动作执行。

应用入口中的可选桌面宠物也改为按能力懒加载，避免不支持该能力的平台提前加载其交互实现。设置搜索属于 config，由应用壳懒加载公开控件并传入 PageHeader 的插槽；页头不依赖 config 实现。跨页面链接拦截属于应用编排，WebPreviewProvider 归入 app。聊天工具栏偏好归聊天 Feature，存储键和版本保持不变。插件只公开工具标签与所需类型，聊天事件与 API 明确声明公开范围，避免通过聚合入口加载页面。

## 移动映射与调用方

| 原路径 | 新路径 / 接口 | 调用方 |
| --- | --- | --- |
| `src/features/workflows/PreviewPages.tsx` | `src/features/mcp/McpPage.tsx`、`mcp-api.ts`、`mcp-queries.ts`、`useMcpDashboard.ts` | 懒加载路由、构建预算及测试 |
| `src/stores/composer-toolbar-store.ts` | `src/features/chat/composer-toolbar-store.ts` | FocusSession、ComposerToolbarSettings |
| `src/components/WebPreviewProvider.tsx` | `src/app/WebPreviewProvider.tsx` | App |
| PageHeader 直接引入 ConfigSearch | `src/features/config/public-components.ts` → App → searchSlot | App、PageHeader |
| ChatResourcePicker 引入插件内部模型和标签 | `src/features/plugins/public.ts` | ChatResourcePicker |
| workflows 的 `previewPages.*` 文案 | 独立 `mcp` 命名空间 | MCP 页面、i18n 注册 |

这些改动遵循现有业务边界，不将业务状态提升到全局、不更改公共 HTTP 字段，也不搬动 Runtime 服务。保留 MCP 页面内的展示组件共置，因为它们共享页面布局与展示规则；没有为缩短文件机械拆分。

## 验证与回滚

`runtime/tests/http-client.test.mjs` 覆盖非法 JSON、领域解码、空响应、JSON/SSE 错误一致性及移动恢复期间的取消/超时。`mcp-client.test.mjs` 覆盖 Runtime 实际快照、非法字段、无副作用读取、旧查询取消与晚到响应、卸载后重挂、失败不自动重试。测试由既有 `npm test` 入口发现。

聊天行数守卫原本用于防止页面重新吞并生命周期职责，现保留其编排检查，并由 `frontend-boundaries.test.mjs` 检查真实导入图的循环依赖、跨 Feature 公开契约、基础层依赖方向、聊天生命周期所有者及轻量入口的传递依赖。此检查不覆盖动态计算模块路径或运行时注入。移动路径同步到懒加载路由、源码守卫和构建预算，预算阈值不变。

验收运行 `npm run check`、`npm test`、`npm run build`，并检查页面交互。共享 React 改动不代表三端原生构建或设备验收已完成；无需据此声称已定位 Windows Python 内存问题。回滚应成组恢复上述调用方、文件、i18n 注册和测试；保留此前独立提交的中性错误提示。无用户数据迁移或新增依赖。

## 复查条件

增加新的 Feature 消费方、MCP 响应字段或共享查询观察者时，重新核对公开范围、字段验证和取消所有权。Runtime 继承/原型注入、存量宽泛 EntityRecord、其他页面手写请求等债务继续按开发流程台账治理。

## 移动三屏工作区生命周期（2026-10-08）

- **问题与范围：** 移动右屏原先随切屏卸载，导致终端清理桥接关闭进程；内嵌扩展丢弃页面保存操作，文件面板固定使用空闲状态；同步导入右屏使桌面首屏也负担其依赖。
- **移动映射与接口：** `src/components/layout/MobileContextScreen.tsx` 移至 `src/app/mobile/MobileContextScreen.tsx`，由应用层组装资产、插件、终端和公开的 `SessionFilesPane`。目录请求和归一化从布局模块移入 `src/features/chat/api/workspace-entries.ts`，布局纯逻辑仅保留视口和手势。App 按需加载移动布局及右屏，桌面直接组合原侧栏和页面。
- **状态与依赖：** 聊天页仍拥有流与快照协调；通过路由回调向壳提供只读 `{ sessionId, streaming }` 投影，离开时撤销。缺少实时所有者时，右屏引导打开聊天，不允许使用旧状态写入。App 在移动右屏首次访问后保留实例；终端和扩展访问后只隐藏，直到移动布局离开或应用卸载才清理。扩展草稿由插件页持有，保存通过其原有 API；资产与扩展分别注册右屏操作，不覆盖聊天主操作。
- **客户端与验证：** 影响 Web、桌面窄窗、Android/iOS 共用 React 布局；原生桥接、权限、资源和持久化格式不变。`npm run build` 检查预算与 Safari 目标；`npm run test:mobile-shell` 在本机 Chromium 中使用实际组件和隔离接口/桥接，覆盖终端保留与销毁、插件保存、运行/确认竞态、文件刷新、Pad 栏宽、手机触摸切屏及内嵌弹窗的视口定位。该入口在 `.github/workflows/ci.yml` 中于构建后执行；先运行 `npx playwright-core install chromium`，或通过 `PISPER_SMOKE_BROWSER` 指定浏览器。Android/iOS 原生打包与真机交互仍需平台验收，浏览器测试不能替代。
- **体积测量：** 同一 macOS arm64、Node 24.20.0、锁文件和生产构建场景下，首屏静态 JS 为 release 基线 297.27 kB gzip、原功能提交 304.82 kB、修复后 298.62 kB；沿用 299.01 kB 预算。没有修改 Safari 构建目标或扩大预算。
- **迁移与回滚：** 无用户数据迁移；回滚须同时还原 App 路由回调、移动右屏路径与调用方及相关测试，避免留下未接线的运行状态或悬空导入。回滚该修复会重新引入切屏终端关闭和配置丢失，应优先修正后续回归。
