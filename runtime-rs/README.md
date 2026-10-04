# runtime-rs — Pisper Rust 后端

用 Rust 重写 Pisper 的后端:以 crates.io 的 [`pi-rs`](https://crates.io/crates/pi-rs) 为引擎,
替换 Node.js 的 `runtime/` 层(原引擎为 npm 包 `@earendil-works/pi-coding-agent`)。
**对外 HTTP 契约保持不变**,三个前端(React GUI / Rust TUI / 手机 App)无需大改即可接入。

## 切片计划(竖切片,每片可独立验证)

| 切片 | 内容 | 对应 Node 层 | 状态 |
| --- | --- | --- | --- |
| 1 | 服务器骨架 + `/api/health` 契约 + 404 兜底 | `runtime/index.mjs`、`sessions-runtime.mjs`(health) | ✅ 本提交 |
| 2 | 会话宿主:`/api/sessions` 列表/创建、`/{id}/input`、`/{id}/live`(SSE)、`/{id}/abort` | `runtime/runtime/agent-runtime.mjs` 等 | ✅ 本提交 |
| 3 | 模型/Provider 配置:`/api/config`、`/api/providers/{p}/connection`、`/{id}/model`、`/{id}/thinking-level` | `provider-preferences.mjs`、`provider-model-catalog-service.mjs` | ✅ 本提交(列表) |
| 4 | 工具冷热网关 + MCP 宿主(`/api/mcp`、`/api/skills`、`/api/plugins`) | `mcp-service.mjs`、`skills-service.mjs`、`tool-gateway-runtime.mjs` | ✅ dashboard/skills/gateway 策略面;插件产品层与 PATCH 持久化随切片 6 |
| 5 | 会话树/分叉(`/api/session-labels`、`/{id}/derive`) | `session-tree.mjs`、`session-derivation.mjs` | ✅ 本提交 + `/{id}/messages` + 多会话 hosting(switch_session) |
| 6 | 工作流/计划任务/远程(`workflows-schedules`、`remote`) | 对应 services | 待办 |

## 契约来源

- 路由清单:`runtime/http/routes/*.mjs`(15 组)
- 最小客户端面:`src-tui/src/api.rs`(TUI 实际调用的端点即必须先实现的端点)
- 版本握手:`apiVersion: 1`、`minClientVersion: 1`(见 `sessions-runtime.mjs` 的 `/api/health`)

## 运行

    cargo run -p pisper-server
    # 默认 127.0.0.1:5174,PISPER_RS_ADDR 可覆盖
    curl http://127.0.0.1:5174/api/health

## 约定

- 后端零 JavaScript:引擎 `pi-rs`(Rust)+ 本服务(Rust);前端三端契约走 HTTP/SSE。
- 每个端点落地前,先对照 Node 实现写契约测试(形状、状态码、错误结构)。
- Node `runtime/` 在切片 6 完成并验收前保持不动,供对照与回退。
