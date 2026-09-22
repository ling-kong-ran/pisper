# Jev 官方 SDK 接入与决策边界

- 状态：已采纳
- 日期：2026-09-22
- 影响范围：Node Runtime（桌面、Web 服务、Android/iOS 内嵌 Runtime）、决策设置页；现有 Web/TUI 决策响应字段保持兼容。

## 背景与审查发现

原实现自行编写 HTTP 请求、退避和超时：退避完成后未移除取消监听器，读取响应正文失败被吞成空字符串，单次超时会在每次重试重新计时，服务停止未取消在途调用。响应归一化会将大于 1 的 noul 压成 1，且没有对照本次问题检查答案类型与选项，因此非法响应可能变成自动批准。设置页测试连接采用默认 30 秒超时，短于后端等待窗口。

官方提供 MIT 许可的 [`@typesafe-ai/sdk`](https://docs.typesafe.ai/sdk/javascript) 和 [Python SDK](https://github.com/typesafe-ai/typesafe-sdk-python)。Pisper 使用 Node Runtime，选择官方 [JavaScript SDK](https://github.com/typesafe-ai/typesafe-sdk-js)，无需启动 Python。

## 依赖与适配决策

固定引入 `@typesafe-ai/sdk@0.6.0`；来源为官方 GitHub 对应 npm 包，摘要锁定在 package-lock.json。包无运行时依赖，npm 报告解压大小为 209,203 字节，MIT 许可证随运行时依赖保留，发布 ESM/CJS 与类型声明。声明要求 Node ≥20，但 Pisper 的支持范围仍以开发流程文档为准；不能将 SDK 的最低版本当成整个应用的最低版本。仅运行时调用时加载 SDK，不进入浏览器包。

SDK 接管请求序列化、重试、退避及请求级计时。`decision-transport.mjs` 使用其公开 fetch 注入接口适配 TypeSafe、OpenRouter 和完整中转端点，保留 Pisper 的稳定错误码，禁止透传 SDK 的正文、cause 和日志。显式关闭 SDK 日志，避免环境变量开启包含业务内容的调试输出；拒绝 HTTP 重定向及含 URL 凭据的配置。

0.6.0 的已知问题及本地保护：

- [取消时克隆响应可能导致旧 Node 退出](https://github.com/typesafe-ai/typesafe-sdk-js/issues/2)：先用单个网络流完成受取消约束的读取，再将内存 Response 交给 SDK；覆盖真实本机 HTTP 在收到响应头后取消的测试。
- [异常可能带出密钥](https://github.com/typesafe-ai/typesafe-sdk-js/issues/14)：校验非空密钥，并只映射错误种类和状态，不暴露 SDK 消息或 cause。
- [空 Retry-After 跳过退避](https://github.com/typesafe-ai/typesafe-sdk-js/issues/9)：将空重试头移除，其他合法服务端退避提示交给 SDK。

响应限制为 1 MiB，按实际字节流计数并检查 Content-Length，超过上限取消读取且不重试。这是决策接口的输入防护，不是全应用内存预算。整个调用含重试和等待的总时限为 120 秒；用户取消和 Runtime 停止优先结束请求。SDK 的强类型不替代运行时答案校验。

## 职责与移动映射

| 原位置 | 新所有者 / 接口 |
| --- | --- |
| decision-remote-client 的错误定义 | decision-errors.mjs，旧入口继续兼容导出 |
| decision-remote-client 的 HTTP、退避循环 | decision-transport.mjs → 官方 SDK；仅接收端点、认证、归一化请求及取消信号 |
| decision-remote-client 的响应归一化 | decision-response.mjs，从 unknown 校验并对照本次问题；旧入口继续兼容导出 |
| 服务停止标记 | DecisionService 持有停止控制器和在途 Promise 集合，取消后等待收尾 |
| 测试连接与显式决策 HTTP 请求 | decisions 路由持有客户端断开监听器并传入 signal，finally 清理 |
| 设置页测试请求 | 卡片持有 AbortController，重试和卸载取消；客户端等待窗口与后端总时限协调 |

保留 Pisper 的输入校验、配置持久化、网关端点和审批阈值，因为它们属于应用业务；不将它们交给模型 SDK。远端响应按本次问题校验数量、ID、类型、选项、档位、有限概率和用量，非法响应返回既有 bad_response，审批调用方沿既有逻辑回落人工。

## 后续模型抽象

本记录描述 SDK 接入。随后引入的协议注册表、领域契约和审批绑定以[决策模型边界](decision-models.md)为准；旧 decision-remote-client 导出保留为兼容门面，Jev 转换与限制归 decision-jev-adapter。下面“无配置格式迁移”仅指 SDK 接入本身。

## 验证、迁移与回滚

沿用 `runtime/tests/decision-service.test.mjs`，增加 `decision-transport.test.mjs` 和 `decision-routes.test.mjs` 覆盖网关路径、非法答案、SDK 错误脱敏、网络流取消、总时限、响应上限、服务退出及 HTTP 断开。原伪 Response 测试改用标准 Response，以覆盖 SDK 实际读取路径。决策模块仍在现有 checkJs 依赖图内。

需要通过全量测试、类型/lint/i18n/格式检查、前端构建、TUI 协议回归和 SEA 闭包/冒烟测试。真实取消回归另以本机 Node 22.20.0 执行，不能据此宣称完整 Pisper 支持 Node 22。未使用真实 Jev 密钥进行付费推理；模拟服务验证协议和失败处理，不验证模型判断质量。移动端使用同一 JS 实现，无平台专属模型或阈值；本机测试不等于 Android/iOS 设备验收。

无配置格式迁移。回滚需成组恢复 SDK 依赖/锁文件、适配器及调用方；不得回退到将非法概率钳制成批准的旧实现，也不得移除取消/错误校验的行为保护。升级 SDK 时重新评估上述上游问题，确认可移除的适配代码并执行同一契约回归。

### 本次验证记录

macOS arm64、Node 24.20.0：全量 Node 测试、综合检查、生产前端构建、TUI check/151 项测试及 SEA 打包/启动冒烟通过。前端首屏静态 JS 为 289.54 kB gzip，未修改预算。Node 22.20.0 的决策传输回归通过；SDK 及 MIT 许可证已进入 SEA 闭包。移动端未执行设备验证，也未使用真实 Jev 凭据。

npm 官方 registry 的生产依赖审计未报告 SDK 漏洞；现有 officeparser/pdfjs-dist 依赖链仍有 2 项高危记录，未在本次 Jev 接入中升级该独立依赖链。维护风险同时包含上文官方 SDK 已知问题，不能将 npm 审计无记录理解为不存在问题。
