# Rust Runtime 远程连接

- 状态：已采纳
- 日期：2026-10-14
- 范围：桌面 Runtime、Tauri 桌面宿主、Android/iOS 原生远程代理

## 问题与职责

Rust Runtime 原先只返回远程访问占位数据，未启动指纹锁定的 HTTPS 服务。原生移动端仍有二维码配对、Iroh 隧道和 HTTP/SSE 代理，但 PC 无法交付可用的二维码及服务。

`crates/pisper-remote-transport` 持有原 `src-tauri/src/iroh_tunnel.rs` 与 `mobile/pinning.rs` 的传输实现和测试；旧模块路径保留薄适配，原生移动调用方不变。Runtime 与移动端复用 Iroh、ALPN、字节转发和 TLS 握手签名验证，移动端不复制业务逻辑。

Runtime 的 `remote_access.rs` 持有监听器、Iroh、设备连接取消与启停；`remote_tls.rs` 管理证书；`remote_pairing.rs` 实现二维码兑换；`remote_http.rs` 建立独立远程鉴权边界；`remote_ops.rs` 保留局域网申请、PC 审批及设备管理。`remote_discovery.rs` 只广播公开 mDNS 信息。Tauri 不再另起重复的 Iroh 服务，Runtime 随 PC 生命周期统一关闭所有连接。

## 路径与协议

设备仍在原代理数据目录的 `remote-access.json`，证书仍为 `remote-tls.json` 的 `version/cert/key` 格式。Tauri 的 `PISPER_IROH_STATUS_FILE` 所在目录继续存放 `iroh-secret.key`，不复制到代理目录；独立 Web Runtime 的身份保存在自身代理目录。损坏证书或私钥返回错误并保留原文件，不生成替代身份。没有目录迁移、双路径读取或数据复制。

二维码仍采用 v1：`v/name/endpoints/fp/code`，提供 `lan` HTTPS 地址与 `iroh` 节点信息。证书指纹为完整 `SHA256:` 十六进制；配对请求从第一次兑换起就校验固定证书及握手签名。Iroh 可直连时使用直连，无法直连时经公网中继；中继转发加密字节，不接收设备令牌或任务明文。二维码配对不要求 PC 有公网入站端口。

客户端在开放本机桥之前以 15 秒上限完成 Iroh 建连；随后 HTTP/TLS 与 SSE 使用同一 QUIC 连接的独立流，避免每个 TCP 请求重复中继注册和握手。连接已关闭时串行重建连接，不重放已经发送的 HTTP 请求。桥关闭时取消它拥有的所有流和待建连任务。

配对码为安全随机的 8 位 Crockford 字符，5 分钟有效，成功一次即消费。哈希和 ISO 过期时间仍直接读写原文件的 `pairingCode.codeHash/expiresAt`，未使用二维码在重启后仍有效；兑换时与设备登记一起原子写入。设备令牌使用 256 位安全随机数，文件只保存 SHA-256 哈希。失败尝试按源地址限流（Iroh 转发的源地址为回环，限流由隧道用户共享）。设备可以调用任务 HTTP/SSE，不能管理配对、设备、防火墙或本机桌面桥接。审批入口只接受真实 LAN 来源，不把 Iroh 回环转发误判成局域网。吊销设备同时拒绝新请求并取消该设备已有响应流；已发起的 PC 任务仍由会话生命周期管理。

本机管理要求回环来源与回环 Host，浏览器写操作仍遵循同源检查。远程监听器拒绝浏览器 Origin，供原生移动代理使用。网页终端采用独立的本机 Cookie/WebSocket 权限边界，远程原生代理目前不代理终端 WebSocket。

桌面宿主默认 TLS 端口 5174；当开发 Web 已占用 5174 时使用 5175。`PISPER_REMOTE_PORT` 可指定端口，0 用于隔离测试自动分配。`PISPER_REMOTE_RELAY_ONLY=1` 关闭服务器 IP 传输及 LAN 二维码入口；`PISPER_REMOTE_OFFLINE=1` 禁用公网中继。后两者为可重复的网络测试场景开关，不改变数据路径。

## 验证与回滚

`npm run test:remote` 是 CI 入口：共享传输测试、隔离 Runtime 与独立模拟手机，使用禁用公网中继的本机 Iroh 直连。`npm run test:remote:public` 显式使用真实公网中继，两侧同时禁用 IP 传输，二维码不得包含 LAN 或直连地址，防止同机测试绕过公网。该测试需要互联网，不能当作离线 CI 的固定依赖。

GitHub Actions 的 `Remote connectivity` 可手动运行相同的直连与公网测试，使用独立 Linux runner，保留脱敏日志供比较本机与 CI 网络差异。外部中继不可达时该工作流失败，不能自动降级为直连或以重试掩盖结果。

两种集成模式都从实际 PNG 识别二维码，验证错误证书、一次性兑换、错误令牌、权限边界、PC 会话创建、模型回复 UTF-8/JSON SSE、PC/客户端重启、原位置证书与身份不变、设备凭据持续有效、吊销断流和关闭远程服务。模型由隔离的本机夹具响应，不读取个人配置，也不调用真实模型服务。测试进程退出后清理其临时目录。

共享传输的离线回归会将首次 QUIC 握手阻塞 6 秒（超过现有 TLS 客户端的 5 秒建连预算），确认桥在握手完成前不开放；随后两个独立 HTTPS 客户端必须经同一 QUIC 连接完成 TLS 与 SSE。测试不放宽产品超时，也不依赖公网，随 `test:remote` 和远程连接工作流执行。

2026-10-14 本机 Windows 验收：使用当前 TUN 网络和真实公网中继，隔离 PC 与模拟移动端连续 5 轮完整通过，每轮两端禁用 IP 传输，包含 PC 重启和设备重连；另通过离线直连回归。此前本机曾出现中继探测超时和重复握手占用 TLS 预算，云端成功不能替代此项本机验收。此记录证明当次网络中的端到端结果，不等于外部中继永久可达。

公网中继模拟不等于两台物理设备的蜂窝网/家庭 NAT 穿透验收。Android/iOS 继续使用原生扫码、权限、存储与代理入口；桌面编译和共享测试不能替代移动构建或真机检查。mDNS 的实际局域网发现也需要同一网段设备验证。

回滚须同时回滚共享 crate、Runtime 接线、Tauri 启停与前端开关调用。持久化格式未改变，无需迁移或回滚用户数据。
