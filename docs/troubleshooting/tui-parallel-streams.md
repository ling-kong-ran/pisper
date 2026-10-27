# TUI 并行终端与流恢复

场景：在同一工作目录打开两个系统终端，分别启动 `pisper`。两个 TUI 有各自的会话；连接来源可能是同一个桌面 Runtime，也可能各自启动独立 sidecar。这与一个 Agent 会话同时调用两个终端工具不同。

## 网关已无请求，但 Runtime 仍在等待

Responses 网关可能已经发出 `response.completed`，并把推理请求标记为结束，却继续保持 HTTP 响应体打开。Pi 的 Responses 流处理会先接收终态数据，再等待底层迭代器结束；如果一直没有 HTTP EOF，`prompt()` 就不能结算，Runtime 仍显示运行中。此时 Runtime 发往 TUI 的 15 秒心跳仍然正常，因此下面的 TUI 连接超时不能解决这一层的等待。

Runtime 的 OpenAI 请求适配现在会在完整读取 `response.completed`、`response.incomplete`、`response.failed`、`error` 或 `[DONE]` 帧后，转发该帧并关闭 SDK 所读的响应流，同时取消上游剩余响应体读取。成功、输出截断、失败和用量仍交给原 SDK 判定；没有收到合法终态的 Responses 不会被伪装成成功。工具参数的 `.done` 不是响应终态，不触发关闭。此修改同时覆盖 Web、TUI 和使用同一 Runtime 的移动客户端，不改变 HTTP/SSE 字段或存储格式。

`runtime/tests/openai-request-transport.test.mjs` 使用本地网关和两个独立 Runtime 进程，共享同一个隔离工作目录及用户数据目录；一条模型响应已发终态但故意不关闭 HTTP，另一条正常结束。回归验证两边都各自完成、每个模型请求只提交一次、实时状态不再停留在运行中，并验证截断/失败结果、用量、分块 UTF-8 和 LF/CRLF/CR 的终态解析。回滚 `runtime/services/openai-request-transport.mjs` 即可恢复旧的上游读取行为，无数据迁移。

## TUI 与 Runtime 之间的连接停滞

原 TUI 的 SSE 客户端只有建连超时，没有读取超时。已收到 Thinking 后，若连接既不再收到数据也不产生 EOF，消费任务会永久等待，已有的 run 游标重挂机制没有机会执行。另一个终端继续收到数据不能证明这条连接健康。

现在每条响应使用独立的 45 秒读取超时。Runtime 每 15 秒发送注释心跳，心跳和业务数据都会重置计时。因此长思考、长命令及等待审批可以持续超过 45 秒；只有连续三个心跳周期完全没有数据时才触发重挂。此超时不是任务执行上限。

重挂复用原 `runId` 和已收到的 SSE 游标，不重复提交消息、不另开任务，也不主动中止后台 Agent。TUI 显示 `reconnecting stream · n/5`；有事件进展时沿用原有重试预算重置规则，连续恢复失败或 run 已过期时结束本地运行显示并保留已生成内容，显示明确错误。连接失败本身不证明后台任务已经停止，重新发消息前应检查原会话或停止原任务。

TUI 连接恢复不更改 HTTP/SSE 字段、存储格式、Runtime 进程共享策略或模型调用行为。回滚 TUI 的读取超时与重挂提示即可，无数据迁移。

## 验证与限制

`cargo test --manifest-path src-tui/Cargo.toml` 自动执行 `src-tui/src/api/stream_tests.rs` 中的回归：真实回环 TCP/SSE 静默停滞、注释心跳保活、原游标续传、过期 run、重试预算耗尽，以及两个测试进程在同一目录使用独立 TUI 客户端，一个停滞恢复而另一个保持健康。App 测试验证重挂提示保留部分内容、不覆盖其他会话或用户停止状态，并在最终失败后退出 Thinking。

这些测试使用本地模型协议夹具，没有调用真实模型，也没有覆盖上游 Provider 的全部异常。已复现“终态已到但 HTTP 不结束”这一条链路；如果网关连合法终态也没有发送且连接也不关闭，仍需结合上游请求诊断，不能仅凭网关面板无活动请求便假定成功。若仍能复现，应补充启动命令、操作系统、TUI/Runtime 版本、连接来源（`pisper doctor`）和是否能用 Ctrl+C 停止；分享诊断前移除凭据、会话内容与个人路径。

## 确认实际运行的版本

npm 启动器、TUI 和 Runtime 的版本号各自独立。启动器可能复用较新的已安装 TUI，同时使用该 npm 包固定的 Runtime，因此不能只用 `pisper --version` 判断 Runtime 版本；该命令还可能先补齐启动器要求的组件。使用当前源码调试可运行 `npm run tui:dev`，仓库改动不会自动替换全局安装的 `pisper`。连接旧桌面或显式指定 `PISPER_TUI_URL` 时，应同步更新 Runtime，保证它提供 run 游标续传和 15 秒 SSE 心跳。
