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
