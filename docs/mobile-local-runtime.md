# 移动端本机 Runtime

当前 Rust 源码由同一个 `runtime-rs` 库提供桌面、CLI 与 Android/iOS 的 HTTP/SSE 业务服务。移动 App 在后台线程建立 Tokio Runtime，直接调用 `start_embedded`；不再启动 Node、JNI Node 宿主或 NodeMobile framework。

## 状态与依赖

- 原 `runtime-rs/src/main.rs` 的路由、状态与领域装配移入 `lib.rs`；二进制入口仅调用 CLI 生命周期。`runtime_host.rs` 暴露显式目录、认证 token、启动结果及异步关闭句柄。库入口不修改进程环境变量，不调用 `process::exit`。
- Tauri 移动壳通过目标专属 Cargo 依赖使用此库；桌面仍启动独立 `pisper-sidecar`。移动后台线程归 `EmbeddedRuntime` 所有，失败、重试和释放时取消并等待线程结束。
- 保留原 App 私有目录 `local-runtime-data/agent`、`local-runtime-data/workspace`。不迁移或复制用户配置、会话和凭据。
- 同源 UI 仍由随机回环端口提供，随机 token 经进程内启动通道交给壳。认证 bootstrap、`/api/ready`、配对、TLS 指纹及远程代理保持原协议；不再轮询 READY 文件。
- 切换远程模式时本机服务继续提供包内前端。远程服务的能力由远程 Runtime 决定。

## 资源与宿主能力

`npm run mobile:runtime` 生成 `pisper-embedded-runtime.tar.gz`，只包含 React 产物、`runtime.json` 和共享语音 catalog/BPE/许可证资料。`embedded-runtime.json` 使用 schema 2，将 App 版本、宿主 profile、前端和完整资源摘要绑定。旧 schema 1 Node 归档不能被新宿主接受；重装资源不会清理用户数据。

语音模型目录、版本与精度仍采用共享 catalog；原生插件继续管理下载、录音和播放。QuickJS 的移动 ABI 用目标 SDK 的 bindgen 生成，RSA 间接依赖沿用 Runtime 已验证的 `pkcs1 0.8.0-rc.4`，避免新宿主解析到不兼容的 rc.5。

本机移动 profile 提供聊天、模型配置、会话、文件、Skill、Web Search、视觉请求及记忆。Shell/PTY、VCS、MCP、第三方插件执行、多代理、工作流和计划任务不在本机移动开放；导航能力、API 拒绝和工具执行前检查共同实施边界。不能仅靠隐藏按钮关闭后台能力，也不能把电脑子进程工具复制进 iOS 包当成支持。

## 构建与验证

使用 Node 24、Rust、Xcode/iOS SDK 或 Android SDK/NDK。Apple bindgen 显式使用 Xcode libclang 和对应 SDK，Android 使用 NDK 工具链；由 `mobile-rust-env.mjs` 装配，避免继承其他平台残留的编译环境。

```bash
npm run build
npm run mobile:runtime
node scripts/check-mobile-rust.mjs ios
node scripts/check-mobile-rust.mjs android
npm run build:android -- --release
npm run build:ios
npm run test:android:runtime -- emulator-5562
```

`check-mobile-rust` 检查库及整个 Tauri 壳，不能代替 APK/IPA 链接、安装或真机验收。`cargo test --manifest-path runtime-rs/Cargo.toml --lib runtime_host::tests` 在隔离目录检查认证、就绪合同、端口释放、重启及会话保留；移动资源和发布合同由 `npm run test:release` 覆盖。设备验收还应覆盖首启、后台恢复、本机/远程切换、语音与原生权限。

## 回滚

按完整源码提交回滚 Runtime 库、移动壳、资源 schema、Cargo 锁文件和 CI 接线；不能只回滚归档或复制旧 Node framework。发布仍只经统一发布脚本，禁止手动推进 App 版本或标签。本次不改变用户数据目录；旧版本能否读取新业务数据仍需按对应协议验证，资源可由旧安装包重新展开。

Android 额外接入 `ndk-context` 与平台证书校验器的 Application/JavaVM 上下文；Java AAR 必须匹配 Cargo 锁定的 `rustls-platform-verifier-android` 版本，暂存脚本校验摘要并附带许可证。依据：[平台校验器 Android 接入说明](https://github.com/rustls/rustls-platform-verifier#android)。禁止通过关闭 TLS 校验绕过初始化失败。Android 系统目录别名由原生壳解析为同一真实目录，内部快照路径仍执行拒绝链接策略，不迁移数据。
