# 移动端本机 Runtime 设计

Pisper 移动端本机模式运行与 Desktop、Runtime 发行包相同的 Node/Pisper Agent Runtime、
标准 `/api/*`、Provider、会话、HTTP/SSE 协议和 React 界面。它不再包含独立的 Rust 对话
Runtime、`/api/local/*` 协议或移动端专用业务 UI。

用户只选择两种产品模式：

- **本机**：会话、Provider 配置和工作区保存在 App 私有目录，由手机内的 Node Runtime 持有。
- **远程**：App 通过 LAN 或 Iroh 连接已配对的 Desktop Runtime。

公开发布的 GitHub APK、Google Play AAB 和 iOS IPA 都只使用 embedded Node，不包含 rootfs、
`su`、`chroot` 或 rooted Runtime 资产。

## 载体选择

Android 使用 APK/AAB 内置的 `libnode.so`，iOS 使用 App 内置的 `NodeMobile.xcframework`。
两种宿主都在专用后台线程调用 Node，不能占用 WebView 或 iOS 主线程。

壳层始终向 UI 返回同一个 `onDevice` 状态和 `runtimeKind: "node"`。用户从本机切到远程时，
同进程 embedded Node 保持驻留，只改变 WebView 路由；Node embedding 不支持在一个 App 进程中
反复初始化。本机 Provider、会话和工作区始终位于同一 App 私有数据目录。

## 统一架构

```text
Normal Pisper React App
          |
          | standard HTTP + SSE + /api/*
          v
http://127.0.0.1:<random-port>
          |
          v
Shared Node/Pisper Runtime (mobile-embedded profile)
          |
          +-- Pi AgentSession / Providers / sessions
          +-- workspace files / skills / built-in tools
          +-- runtime-derived capability manifest

Android carrier: JNI/C++ -> node::Start(...) -> libnode.so
 iOS carrier: background thread -> node_start(...) -> NodeMobile.xcframework
```

嵌入式 Runtime 使用随机回环端口和随机 bootstrap token。Node 完成真实 Agent Runtime 初始化后，
以 `0600` 权限原子写入 READY 文件；壳层只接受带 token 的
`http://127.0.0.1:<port>/_pisper/desktop/bootstrap` 地址。READY 不通过 stdout 解析，也不暴露到
共享存储。

## 能力清单

同一 Runtime 不等于所有宿主都能提供同一系统能力。`GET /api/runtime/capabilities` 返回实际宿主
清单；`node:child_process`、`node:worker_threads`、`node:sqlite` 和 WASM 等底层能力来自真实探测。
宿主 profile 可以保守关闭不适合该载体生命周期或缺少壳层 bridge 的服务，但不能伪造缺失模块。

| 能力 | Android / iOS embedded |
| --- | --- |
| React、Provider、标准会话、流式聊天 | 支持 |
| 工作区读写、资源与 Skill | 支持 |
| 内置工具目录与 Web Search | 支持 |
| Shell、Git/SVN | `child_process` 缺失或受渠道限制时关闭 |
| Desktop PTY 终端与浏览器自动化 | 关闭 |
| 记忆 | `node:sqlite` 缺失时关闭 |
| 第三方插件执行 | 关闭；内置工具设置仍保留 |
| MCP stdio | 关闭 |
| Goal、Plan、多 Agent、工作流、计划任务、渠道 | 关闭 |
| 图片处理 | 由实际 WASM 探测决定 |
| Desktop 远程访问管理、桌面宠物 | 关闭 |

React 导航、请求发起、工具目录和服务端 API 都只对清单中**明确为 `false`** 的能力执行隐藏或
HTTP 409 拒绝。没有能力端点的旧 Runtime 保留原有导航行为，避免破坏远程兼容性。

## Runtime 闭包

embedded Node 不能执行 SEA 内嵌的 JavaScript blob，因此 App 单独打包可由 Node 直接执行的
Runtime 闭包。`scripts/stage-runtime-closure.mjs` 与 SEA 共用依赖裁剪和关键闭包审计，移动闭包额外
移除桌面 clipboard、TUI 和不适用的原生包，并包含：

- `runtime/mobile-embedded.mjs`；
- 完整的 Pisper Runtime、Pi Coding Agent 依赖和共享模块；
- 生产 React `dist/`；
- 与 App 版本绑定的 `embedded-runtime.json`。

安装使用临时文件、staging 目录和原子目录替换。入口或 React 资源缺失、归档过大、版本不匹配时
不会替换已安装版本。

## Node 供应链

移动 Node 24 固定到 `ChamHerry/nodejs-mobile` commit
`8a995e179bb2c224029a560ae9c4f9460631b94d`，Node `24.18.1`，modules ABI `137`。
`scripts/mobile-node-artifacts.json` 记录 recipe/materialized tree、Android Release 归档、Sigstore bundle
和 `libnode.so` SHA256。

- Android 发布构建必须验证归档摘要、Sigstore workflow 身份、内部 manifest 和 `libnode.so` 摘要，
  再把 headers 与 arm64 库交给 CMake。
- iOS 在 macOS 从固定 commit 和固定 materialized tree 构建 `NodeMobile.xcframework`，记录 Xcode
  版本与逐文件 SHA256。
- embedded Runtime、iOS Node framework 归档、APK 和 unsigned IPA 都使用项目 Minisign 密钥
  签名。任一 Android/iOS 构建、签名或资产检查失败时，不发布 `app-v*`。

`npm run release -- patch` 会按 App 独立路径和最新 `app-v*` 标签自动检测是否需要发布，并在同批
TUI、Runtime、npm 与 Desktop 工作流全部成功后，最后派发 App 工作流。

## 数据与安全边界

本机会话、Provider 配置、工作区和 Runtime 状态位于 App 私有数据目录。Node/Pisper 使用标准数据
格式，因此不是旧移动 Runtime 的 `providers.json` / `sessions.json` 私有协议。当前 Provider 凭据和
远程配对令牌依赖 App 沙箱与系统文件权限保护，尚未迁入 Android Keystore 或 iOS Keychain；不要
导出 App 私有目录，设备丢失时应立即在 Desktop 端吊销对应远程设备。

Runtime 仅监听 `127.0.0.1`，bootstrap token 不经普通 API 回显。模型请求仍会把完成任务所需内容
发送给用户选择的 Provider；本机运行不等于模型调用端到端加密，也不能替代设备锁屏、系统更新、
可信侧载来源和可靠备份。

## 已知平台边界

- Node.js Mobile 不提供 `child_process` / `cluster`；缺少这些模块时 Shell、终端、VCS 和 MCP stdio
  会从 UI 与 API 中同时关闭。
- iOS 不能在 WebView/main thread 运行 Node，且操作系统可能冻结后台 App；READY 和 SSE 恢复不能
  绕过系统生命周期限制。
- 生产内置 Runtime 目前仅支持 arm64。x86_64 Android 模拟器不能代替 arm64 真机启动验证。
- iOS IPA 不含 Apple 分发签名，必须由使用者自行重签名后安装。
