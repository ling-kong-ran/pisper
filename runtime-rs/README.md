# Pisper Rust 后端

原生 HTTP/SSE 服务以固定版本 Pi 0.2.2 为引擎，替换本分支已移除的 Node Runtime。
React 与 Rust TUI 共享配置、会话快照和聊天协议。Windows 桌面通过 Tauri 启动同一服务。

## 当前能力

- 模型连接向导、显式 HTTP 模型发现、凭据保存、默认模型和思考等级。
- 新建、重命名、整理、删除会话；完整消息、工具结果、分页和会话树读取。
- 流式聊天、重连回放、工具执行、停止、追加输入、切换会话和重启恢复。
- 原生 MCP 连接及工具、配置管理、实际连接测试；技能目录。
- 本机界面偏好与 Dock 布局持久化、浏览器通知队列。
- 工具审批等待、拒绝、批准、停止与超时；文件修改展示差异并复核原文件。
- SQLite v4 记忆空间、手动记忆、搜索、候选审核和图谱持久化。
- 资产上传、去重、多会话引用、文件预览与 Range 下载。
- 原生计划、持续目标、预算总结、团队任务图、独立子 Agent 与实际执行队列。
- 工作流 DAG、审批、重试、取消、媒体输入和手动/定时计划任务。
- 原生视觉 Provider、固定摘要的图片引擎下载、U2Net CPU 背景处理和 Telea 修复。
- Sherpa ASR/MeloTTS、热词、语音模型校验与下载、流式识别和独立 Rust Worker 取消。

审批模式会暂停需授权的工具，用户批准后才执行；拒绝、停止或超时不会执行工具。
工作区自动授权仍校验目录边界与高风险操作；完全访问使用忽略通用审批模式。
公开会话和子 Agent 使用独立原生会话，可并发运行；同一会话的重叠操作返回
`409 session_busy`。目标模式在轮次间保留运行所有权，配置变更不会穿插到活动任务中。
子 Agent 完成通知保留持久邮箱，只有真实 JSONL 写入回执后才确认送达。

完整 release 等价仍未完成。远程控制、桌宠、外部登录导入、插件策略、Custom UI 执行、
逐会话文件快照审核和撤销、外部通知通道、手动图片帧/笔刷编辑及跨平台原生验收仍有缺口。
文件读写工具仍可正常使用。能力接口关闭对应入口，操作返回结构化未支持错误，
不会把已有的简化内部数据当成成功结果。已接入服务的能力标记表示入口可用，
完整兼容情况仍以 `docs/reports/rust-release-parity*.json` 的行为与平台证据为准。

## 数据与协议

默认目录沿用 `~/.pisper/agent`，可由 `PISPER_AGENT_DIR` 或 `PI_CODING_AGENT_DIR` 显式覆盖。
`PISPER_RS_DATA_DIR` 单独覆盖产品偏好目录；默认与 agent 目录一致。
模型、鉴权、设置及原应用 JSON 的未知字段被保留，配置响应不含 API 密钥。
历史兼容 sessions 根目录及 Pi 的工作区子目录，查看快照不会切换正在工作的引擎。

`GET /api/sessions/{id}/live` 默认是 JSON；明确请求 `Accept: text/event-stream` 的旧客户端
仍可订阅事件。`POST /api/chat` 返回可重连的 SSE，终止帧在 prompt 和持久化完成后发送。
未知 `/api/*` 返回 JSON 404；前端静态路由仍回退到 index.html。
HTTP 业务错误使用 `{ "error": "可展示的错误消息", "code": "稳定错误码" }`。

Pi 发布版本的原生 MCP 修补及来源记录见 [PISPER-PATCHES.md](vendor/pi-rs/PISPER-PATCHES.md)。

## 构建与验证

在仓库根目录运行：

```powershell
cargo test --locked --manifest-path runtime-rs/Cargo.toml
cargo build --locked --manifest-path runtime-rs/Cargo.toml
node scripts/smoke-rust-usability.mjs runtime-rs/target/debug/pisper-server.exe
npm run check
```

验收脚本使用隔离目录、合成凭据及本机模型/MCP 服务，通过真实界面配置连接和发送消息，
再检查工具、消息历史、会话切换、错误和重启。未支持功能单独报告 `UNSUPPORTED`，
不计作这些功能的成功验证。`npm run check` 的启动闸门覆盖原生服务的隔离使用链路。

## Windows 安装包

需要 Node.js 24、Windows Rust 工具链及 Visual Studio C++ 构建工具：

```powershell
node scripts/package-tauri-rust.mjs
```

入口构建生产前端、执行契约和使用链路检查、编译 Rust 后端及 TUI，并生成 NSIS 安装包。
产物位于 `release/tauri-rust-artifacts/windows-x86_64/`。后端使用独立输出位置，
不覆盖已运行的开发服务器；本地构建沿用分支的版本元数据。
语音原生 DLL、词表与许可证通过 `scripts/stage-rust-speech.mjs` 校验后打入
`sidecar-runtime/speech-native/` 和 `sidecar-runtime/shared/`，安装后不依赖 Node 模块。
较大的 ASR/TTS 模型不随安装包分发，继续使用原代理目录的 `speech-models/`。

桌面壳通过 `PISPER_FRONTEND_ROOT` 指定安装后的前端目录。后端监听随机本地端口，
完整 `PISPER_SIDECAR_READY` 消息交付引导 URL 和 PID。引导校验令牌并设置 HttpOnly Cookie，
后续请求使用同源 Cookie 鉴权；TUI 可通过 `PISPER_DESKTOP_TOKEN` 传入令牌。
`PISPER_DESKTOP_DATA_DIR` 是独立桌面测试目录，正常启动仍使用系统的应用数据目录。
