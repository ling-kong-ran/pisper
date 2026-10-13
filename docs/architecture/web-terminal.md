# 桌面与网页共享 PTY

- 状态：已采纳
- 日期：2026-10-14
- 范围：React、桌面 Rust Runtime、Tauri；移动本地宿主不提供终端

## 背景与决定

终端入口此前依赖 `window.pisperDesktop.terminalProfiles`，因此桌面浏览器无法显示已有终端组件。复用原生 PTY 的进程树、UTF-8 字节流和退出语义，避免为网页复制另一套 Shell 生命周期。

原 `src-tauri/src/desktop_shell/desktop_terminal.rs` 的进程核心及原有测试移动到 `crates/pisper-terminal/src/lib.rs`。原路径只保留 Tauri 命令适配及主窗口权限检查，命令名和 IPC 负载不变。Runtime 的 `terminal_api.rs` 依赖共享 crate，提供 `/api/terminal/profiles` 和 `/api/terminal/connect` WebSocket；移动端不链接 PTY crate，也不注册这些路由。

每条 WebSocket 拥有一个终端实例。首帧 `create` 携带会话 ID、Shell profile 和尺寸；工作目录由会话服务解析，禁止客户端提交可执行文件或 Shell 启动参数。后续 `input` 传字节数组，`resize` 传行列，`close` 结束实例。响应依次为 `created`、既有 `output` 字节事件和 `exit`/`error`。无自动重放或重试，避免重复执行命令。

HTTP 沿用桌面 Cookie 鉴权，升级额外要求同源 Origin；没有桌面令牌时仅允许本机 Host。浏览器刷新、关闭、协议错误、输出过载及 Runtime 停机都会关闭对应 PTY 并回收进程树。全局连接上限为 12，启动握手和发送有超时，输出队列有界。其他页面的终端不受某个面板关闭影响。

前端面板继续拥有 xterm、会话标签、尺寸观察器和事件订阅，`terminal-transport.ts` 按运行环境选择完整原生桥接或同源 WebSocket。终端面板第一次打开时才加载，收起后保留进程；关闭标签或离开页面才结束进程。偏好继续使用 `pisper-terminal-panel`，没有新增数据目录或迁移代码。

## 验证与回滚

`npm run test:terminal` 运行共享 PTY 原测试并以隔离 Runtime 和真实浏览器验证网页终端。CI 覆盖 Linux 网页流程与 Windows 进程生命周期。旧原生冒烟的证据检查接受共享 crate 的测试路径，原保护目标（真实 OS 子进程与回收）不变。Tauri 命令及原生 UI 冒烟继续适用。

按同一提交回滚适配、Cargo 依赖与入口即可恢复仅原生终端。没有用户数据格式变更，无需任何数据回滚。iOS/Android 本地不支持 PTY，不将桌面检查描述为移动真机验收。
