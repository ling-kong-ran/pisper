# 移动端后台恢复与远程连接验证

本次验证基于 `release` 分支 `167a53c05444ea7239bbf0146584faf67ed854bd` 加本地工作区修复，日期为 2026-09-08（UTC+8）。提交前已同步远端 `b096187` 的截图和宣传视频更新；该更新未改变产品代码，相关首页与移动资源测试 34 项通过。App 清单版本为 `0.1.33`；本地测试构建不代表已发布新版本。

## 平台与入口

| 项目 | 桌面 | Android | iOS |
| --- | --- | --- | --- |
| 前端 | `RemoteAccessSettings.tsx` 管理监听和防火墙 | 共用 `mobile-runtime-recovery.ts`、`http.ts`、`ChatPage.tsx`、`RouteErrorBoundary.tsx` | 与 Android 共用 |
| 原生与代理 | Runtime HTTPS + Rust Iroh 服务 | `mobile/mod.rs` 的 `RunEvent::Resumed` 与 `mobile_resume_local_runtime`，`mobile/proxy.rs` | 与 Android 共用；WebKit WebContent 回收回调 |
| 平台生命周期 | 开关远程访问与服务关闭 | Android WebView renderer 回收、HOME/前台切换 | WebContent 进程回收、Preferences/前台切换 |
| 资源与依赖 | 独立测试目录中的真实 Desktop Runtime | NodeMobile、Rust/Iroh、最新 embedded Runtime 与 React 静态资源 | 正确的 arm64 simulator NodeMobile slice、Swift 资源 bundle、相同 Runtime 和前端 |
| 本轮环境 | macOS 26.6.2；隔离的 Node Runtime 和 Rust relay 服务 | Android 16/API 36 arm64 模拟器；R8 release APK | iOS 26.5 arm64 模拟器；Xcode release 构建 |

本轮未增加系统权限或平台专属业务分支。三端共用配对、TLS 指纹校验和数据合同；App 的 UI 始终由自身签名包中的资源提供。

## 已确认的修复

- Vite 预加载错误和脚本加载失败可进入原生恢复；原生命令拒绝、挂起或未产生导航时，20 秒 watchdog 执行同源重载，并保留路由。60 秒冷却避免确定性资源损坏造成刷新循环。
- API 的取消与超时覆盖恢复模块加载及原生等待；连续切后台采用 generation 隔离，旧恢复不能覆盖新一轮检查。聊天恢复和错误页手动重载共用有界入口。
- LAN 探测最多并发 8 个，共享 2.5 秒预算；Iroh 独立获得 8 秒探测时间，请求交付总预算为 25 秒。旧 HTTP 连接与过期世代隔离，并发解析共享结果；业务 500/408 不清理整个连接池，已经发送的 POST 不重放。
- 前台/online 恢复通知 Iroh 重新检查网络，最多等待 1 秒。确认直接地址与 Iroh 均发生传输故障后，共享一次后台 Endpoint 重建，完整退休旧桥与 Endpoint，再用原密钥恢复节点身份；30 秒冷却避免并发重建。HTTP 状态错误、配置错误和指纹拒绝不触发重建，健康 LAN 不等待 Iroh 恢复。
- 桌面开启远程访问时管理实例专属防火墙策略，并展示真实结果及授权/清理重试。启动与轮询只读；远端 Bearer 请求不能触发系统授权。

## 设备验证方法

所有本次日志与产物位于 `release/resume-validation/`。验证脚本只临时修改已安装 Runtime 的 `dist/index.html`，追加随机 loopback 控制脚本；原始文件在 `finally` 恢复，产品源码和签名包不包含测试控制入口。

1. 终止 renderer/WebContent，检查主进程存活、React 页面和本地 Runtime 恢复。
2. 向真实 WebView 注入 `vite:preloadError`，观察页面替换和就绪状态。
3. 注入恢复事件后实际暂停原生进程 22 秒，验证 20 秒 watchdog、导航和原生恢复。
4. 使用真实配对 API 连接隔离 Desktop Runtime。端点列表包含 12 个接受 TCP 后保持静默的 LAN 候选及一个正常入口。
5. 关闭正常 LAN 入口的现有连接并静默丢弃后续流量，检查真实公网 Iroh relay 路径。relay 服务禁用全部直接 IP 传输，因此本轮 `iroh` 结果确实经过公网 relay。
6. 在远程模式切后台，并用 SIGSTOP 暂停原生进程 65 秒，再 SIGCONT 和激活 App，检查 API、页面、主进程与传输状态。
7. 注入 online 事件，恢复 LAN 并验证切回直连，最后返回本地模式。

这是模拟器故障注入；不等于 Android/iOS 真机、蜂窝网络或数小时待机验收。静默 LAN 入口用于模拟不可达路径，并未修改宿主机的真实防火墙。

## 已完成检查与发现

- 最终 `npm test`：1,928 通过，2 跳过，0 失败，包含新增的手动重载行为测试。
- 最终前端 `npm run check` 与 `npm run build` 均通过，包含 bundle budget。
- 最终 Iroh 重建与 LAN 隔离补充后的 Rust：81 项单元测试、2 项集成测试通过，3 项公网/手动测试默认忽略；严格 Clippy 通过。定向测试覆盖并发恢复、取消等待、旧世代保护、真实 TLS 连接和节点身份保持，以及恢复期间健康 LAN 的缓存命中和新探测。
- TUI：`tui:check` 通过，151 项测试通过。
- Android 最终 R8 release APK 已真实安装，全部 10 个 arm64 原生库通过 16 KB 页面大小检查。完整设备用例通过，65 秒冻结后约 5.8 秒恢复 relay。
- iOS 最终包已完成新 arm64 simulator 构建、安装、NodeMobile slice/资源/归档哈希校验及真实 React 启动。完整设备用例通过，65 秒冻结后约 5.9 秒恢复，页面与主进程保留。

| 最终完整用例 | Android R8 release | iOS Xcode release simulator |
| --- | --- | --- |
| renderer / WebContent 回收 | 通过；主进程存活 | 通过；主进程存活 |
| Vite 预加载失败 | 约 1.0 秒重载 | 约 0.6 秒重载 |
| 原生进程暂停 22 秒 | 约 23.1 秒恢复 | 约 22.4 秒恢复 |
| 12 个静默 LAN 候选后可用连接 | 通过 | 通过 |
| LAN 全部被丢弃后转公网 relay | 约 4.7 秒 | 约 6.0 秒 |
| 65 秒后台冻结后恢复 relay | 约 5.8 秒 | 约 5.9 秒 |
| online 事件和恢复 LAN 后切回直连 | 通过 | 通过 |

结果分别为 `android/results.json` 和 `ios-endpoint-full-results.json`。耗时包含测试脚本轮询与状态核验，不代表性能承诺。

### 增强故障用例仍有恢复延迟

旧版失败记录在 `ios-full-native-trace.log` 和 `ios-failure-trace.txt`：健康探测曾在约 6.9 秒成功，但后续多个连接约 5 秒后失败，随后出现 502、504 和更长恢复延迟。这些记录没有因最终正常用例通过而删除。

最终包还额外执行了“所有 LAN 不可用 + Iroh 内 TLS 转发静默断流 7 秒”的设备用例。原生日志 `ios-rebuild-trace.log` 确认 `recovery_start → recovery_closed → recovery_end`，同一请求在约 13 秒后经公网 relay 返回 200，证明设备实际执行了新增的后台重建分支。节点身份保持及并发只重建一次另由真实 TLS Rust 测试验证。

**但在这次重建后紧接着再冻结 65 秒，首个请求仍返回 504，下一次诊断请求返回 502，约 57 秒后才重新获得 200。这个增强用例整体未通过首次请求就绪断言。** 主进程与页面继续运行，后台重建确实发生；这不能被表述为 iOS 在所有连续故障下都能立即恢复。后续若继续优化，需结合公网 relay 与 QUIC 建连阶段日志定位重建后的路径恢复延迟，保留当前取消、冷却、TLS 校验和不重放 POST 的边界。完整记录为 `ios-rebuild-smoke.log`、`ios-rebuild-last-observation.json` 和 `ios-rebuild-trace.log`。

## 纯公网首次配对补验

按用户重点补充了从首次配对开始只走公网 relay 的测试。桌面状态接口、二维码和 App 保存的档案都只有 `iroh` 端点，服务端调用 `clear_ip_transports()` 禁用直接 IP 传输；Android 没有建立通往桌面端口的 adb reverse，只有验证脚本自身的控制端口转发。

| 场景 | Android R8 release | iOS Xcode release simulator |
| --- | --- | --- |
| 仅公网地址首次配对及认证 API | 通过，约 3.6 秒 | 通过，约 3.9 秒 |
| 公网连接下冻结 65 秒后恢复 | 通过，约 4.4 秒；页面保留 | 通过，约 4.5 秒；页面发生恢复重载 |
| online 恢复 | 通过 | 通过 |

Android 额外在模拟器内部对 Pisper UID `10214` 设置 IPv4/IPv6 的 UDP 拒绝规则，保留 DNS；IPv4 规则实际命中 35 个包，连接仍通过公网 relay 完成。两条规则均已删除，宿主 Mac 防火墙未变。iOS 没有执行同等 UDP 限制，因为 iOS simulator 使用宿主网络栈，本轮不修改宿主防火墙。

这证明可以在当前环境实测公网 relay、纯公网首次配对和 Android 受限 UDP 出站。两端仍共用 Mac 的互联网出口，不能据此宣称已验证两个独立 NAT/运营商 CGNAT 下的 UDP 穿透，或真实 Wi-Fi/蜂窝网络切换。前述连续故障用例约 57 秒恢复的限制仍然有效。

证据：`release/resume-validation/android-public-only/results.json`、`ios-public-only/results.json`、`android-public-only-network.json`，以及同名 `.log` 和 `.status.json`。

## 防火墙验证边界

防火墙服务及路由采用注入的系统命令测试，覆盖 Windows、macOS、UFW/firewalld、授权取消、所有权迁移、清理和来源校验。没有在本轮实际提权或修改 Windows/Linux/macOS 系统规则。

宿主 macOS 防火墙只读查询为关闭；隔离 fixture 使用通用 Node，服务正确返回 `unsupported / packaged_app_required`。打包的 macOS sidecar 的实际系统授权、Windows/Linux 实机规则和手机到真实宿主防火墙的端到端放行，均未实测。

## 本地产物

- Android：`release/resume-validation/app-release-test.apk`，使用临时测试签名，供本地验证；不是商店或正式发布签名。
- iOS：`release/resume-validation/ios-native-Pisper.app`，仅 arm64 simulator，不能安装到真机。
- 构建和资源核验：`android-artifact.json`、`ios-native-result.json`、`ios-native-artifact.json`。
- 本轮工具：`device-smoke.mjs`、`desktop-fixture.mjs`、`android-rebuild.mjs`、`ios-native-build.mjs`。这些位于 gitignored 的本地验证目录；可保留用于本机复验。

既有的 `docs/mobile-startup-verification.md` 在本次开始前就存在，本轮保留原样，其旧结果不作为本次证据。

验证结束后，已恢复两端已安装的 `index.html`，其 SHA-256 与最终前端构建逐字节一致；移除测试配对档案、重启到本地模式，并停止隔离 Desktop Runtime、TCP 故障入口和两个公网 relay 测试进程。核验见 `release/resume-validation/cleanup.json` 和 `public-only-cleanup.json`。没有修改宿主系统防火墙。
