# 内置 MCP 服务的边界与生命周期

- 状态：已采纳
- 日期：2026-09-25
- 影响范围：桌面与源码 Runtime、React Web 管理界面、SEA 分发闭包；TUI 共用 Runtime 但不新增命令；Android / iOS 本机 Runtime 不开放此能力。

## 背景

原有 `runtime/services/mcp-service.mjs` 和 `src/features/mcp/` 实现的是 Pisper 作为客户端连接外部 MCP 服务。让其他 Agent 调用 Pisper 是反向连接，具有不同的监听、鉴权、令牌和工具权限边界。若直接复用桌面 Runtime 的普通 HTTP 接口，配对设备可能取得过宽的管理权限，且外部客户端需要理解 Pisper 私有 API。

## 决策与职责

没有移动现有出站 MCP 代码。新增职责如下：

| 位置 | 职责 |
| --- | --- |
| [`runtime/services/mcp-host-service.mjs`](../../runtime/services/mcp-host-service.mjs) | 拥有启用状态、独立令牌、监听端口、请求与连接生命周期；默认关闭，只绑定 127.0.0.1，启用时才加载 MCP SDK。 |
| [`runtime/services/mcp-host-tools.mjs`](../../runtime/services/mcp-host-tools.mjs) | 公开固定的 Pisper 工具目录，校验参数与限制结果，调用 Runtime 已有的会话、记忆、工作流等入口。 |
| [`runtime/runtime/mcp-host-adapter.mjs`](../../runtime/runtime/mcp-host-adapter.mjs) | 把 Agent Runtime 的会话、记忆与工作流等能力收敛成显式接口；服务层不持有整个 Runtime 对象。 |
| [`runtime/http/routes/mcp-host.mjs`](../../runtime/http/routes/mcp-host.mjs) | 为本机管理界面提供状态、启停、读取及轮换令牌；拒绝配对远程设备及非本机请求。 |
| [`runtime/app-runtime.mjs`](../../runtime/app-runtime.mjs)、[`runtime/http/api-handler.mjs`](../../runtime/http/api-handler.mjs) | 装配服务、按设备能力恢复已启用的监听、在 Runtime 停止时先关闭监听并接入管理路由。 |
| [`src/features/mcp/`](../../src/features/mcp/) | 在既有 MCP 页面展示独立的内置服务开关与连接配置；普通状态响应不包含令牌。 |

管理界面通过 Pisper 原有 HTTP 客户端调用管理路由，外部 Agent 通过独立的 MCP Streamable HTTP 端口调用公开工具。MCP 服务只在用户开启后监听固定回环地址，使用独立 Bearer 令牌；服务端同时检查请求来源、Host 与 Origin。固定地址便于外部客户端保存配置。令牌以受限权限写入 Agent 数据目录的版本化状态文件。`GET` 状态无令牌；只有本机管理操作按需读取完整连接信息。令牌轮换会关闭旧连接并使旧令牌失效。

工具目录是显式白名单。发送消息通过原会话通道进入 Agent Runtime，工具执行继续使用该会话的权限及审批；工作流和计划任务保留原有领域规则。外部协议不暴露批准权限请求、读取 Provider 凭据或任意内部方法的入口。结果有大小上限，并在返回前做现有格式的敏感文本脱敏。新增字段和端点不更改现有 Web/TUI 的 HTTP 或 SSE 负载。

## 替代方案

- `stdio` 服务需要一个独立进程，还要解决与运行中 Pisper 会话状态的连接和生命周期；首批实现采用同进程的 Streamable HTTP。
- 把主 HTTP API 直接包装为 MCP 会复用错误的信任边界，也无法给外部工具提供稳定的输入契约。
- 通过桌面远程访问向其他设备发布 MCP 会扩大可达范围；当前需求是本机其他 Agent，故不开放此路径。

## 平台、验证与兼容

桌面端复用现有 React MCP 页面与 Node SEA Runtime，原生 Tauri 命令、权限及资源不变；SEA 闭包需包含 MCP Server SDK。TUI 共用同一 Runtime，没有新增 CLI 命令或线协议字段。Android 与 iOS 共用 React 源码，但其 embedded Node 能力清单将本机 MCP 标记为不可用，不启动监听；连接桌面端的配对设备也不能管理服务。手机端不增加原生依赖、权限或资源打包，不宣称支持此服务。

[`runtime/tests/mcp-host.test.mjs`](../../runtime/tests/mcp-host.test.mjs) 应覆盖默认关闭和重启恢复、鉴权及远程拒绝、启停/轮换、SDK 客户端发现与调用、审批中的异步消息、端口冲突和 Runtime 停止。还需运行 `npm run check`、`npm test`、`npm run build`、`npm run tui:check` / `npm run tui:test` 及 SEA 冒烟；设备安装与实际外部客户端连接应按交付环境另行确认，不能由源码测试推断。

## 迁移与回滚

没有既有 MCP 主机配置需要迁移。状态文件采用版本 1；无效或未知内容按关闭处理，不自动开放端口。回滚时关闭服务并退回旧 Runtime 即可；保留状态文件不覆盖用户数据，但旧版不会启动 MCP 监听。再次升级时，只有有效且明确开启的配置才恢复监听。未来如需扩展到网络访问、增加危险操作工具或支持移动端，须重新评估鉴权、审批和平台能力，不靠扩大现有端口绑定完成。
