# Rust Runtime 数据目录契约与 #34

对照基线为 Node 桌面版 `v0.5.81`，核对范围为应用启动入口、领域存储、Pi 配置、桌面/TUI 与移动端宿主。下表中的相对路径均位于原数据根目录内。

## 唯一数据根目录

- 桌面、Web、TUI：默认 `~/.pisper/agent`；只有 `PISPER_AGENT_DIR` 可以覆盖，相对值按进程工作目录解析。
- `PI_CODING_AGENT_DIR` 由 Pisper 向 Pi 引擎传递，不能反向覆盖 Pisper 配置；`PISPER_RS_DATA_DIR` 不能将领域数据拆到另一个目录。
- 未指定工作目录时，沿用 Node 的用户主目录；启动进程所在的安装目录不是用户工作目录。
- Android、iOS：沿用宿主应用数据沙箱内的 `agent` 和 `workspace`。`src-tauri/src/mobile/embedded_runtime.rs` 显式传入相同路径，不使用桌面主目录规则。
- 首次启动先创建缺少的数据根目录，再打开要求目录存在的插件等服务。已有目录和文件不移动、不复制、不重命名。

## 原数据源清单

| 数据 | 原路径 / 结构 | Rust 所有者 |
| --- | --- | --- |
| Provider、凭据、首选项 | `models.json`、`auth.json`、`settings.json`、`pisper.json`、`pisper-provider-keyrings.json`、`pisper-provider-models.json` | `provider_config` / Pi 配置层 |
| 模型元数据与长期窗口参考 | `pisper-model-metadata.json`、`pi-dev-models-cache.json` | `provider_config/model_metadata`（原地只读） |
| 会话与整理状态 | `sessions/*.jsonl`、`pisper-sessions.json`；置顶、归档、未读、失败和完成时间位于 `organization` | `session_api`、`session_read_cache` |
| 扩展包 | `settings.json` 的 `packages`；项目配置为 Pi 的 `.pi/settings.json`，npm/git 安装位置由同一 Pi 包管理器解析 | `extension_packages` |
| 技能和提示词 | `pisper-skills.json`、`skills/`、`prompts/`，项目 `.pisper/skills` 与 Pi 资源搜索路径 | `skills_ops` / Pi 资源层 |
| 聊天布局、浏览器偏好 | `pisper-chat-dock-layout.json`、`pisper-browser-preferences.json` | `ui_contract` |
| 资产与重试队列 | `pisper-assets.json`、`pisper-assets/`、`pisper-workspace-asset-retries.json` | `native_assets` |
| 记忆与用量 | `pisper-memory.sqlite`、`pisper-usage.json` | `memory_store`、`native_memory` |
| 目标、计划、代理、团队、审批 | `pisper-goals.json`、`pisper-plans.json`、`pisper-agents.json`、`pisper-teams.json`、`pisper-approvals.json` | 各领域服务，由 `lib.rs` 注入路径 |
| MCP 配置与宿主 | `pisper-mcp.json`、`pisper-mcp-host.json` | `mcp_api`、`mcp_host_ops` |
| 通道与通知 | `pisper-channels.json`、`pisper-browser-notifications.json`；通知设置仍在 `pisper.json` | `native_channels`、`native_notifications` |
| 工作流、计划任务 | `pisper-workflows.json`、`pisper-schedules.json` | `workflow_engine`、`schedule_api` |
| 插件 | `pisper-plugins.json`、`plugins/`、`plugin-data/`、`plugin-sources/` | `native_plugins` |
| 决策配置 | `decisions/config.json` | `native_decisions` |
| Custom UI | `custom-ui/<id>/manifest.json` 及组件资源 | `native_custom_ui` |
| 图像与游戏资产 | `game-assets.json`、`game-assets/`、`image-tools-agent/`、`image-operation-jobs/` | `native_game_assets`、`native_image_agent` |
| 文件变更快照 | `file-change-snapshots/` 中的索引与原文件快照 | `native_file_changes` |
| 语音、模型与引擎 | `speech-settings.json`、`speech-models/`、`speech/`、`sprite-engines/` | `native_speech`、`native_workflow` |
| 桌面宠物 | `desktop-pet.json`、`desktop-pets/`；外部宠物仍来自用户主目录的 `.petdex/pets`、`.codex/pets` | `desktop_ops` |
| 远程配对 | `remote-access.json`，保留 `version/enabled/devices/pairingCode`；设备存 `tokenHash` 和 ISO 时间，吊销写 `revokedAt` | `security`、`remote_ops` |
| 防火墙与赞助缓存 | `remote-firewall.json`、`sponsors-cache.json` | `firewall_ops`、`desktop_ops` |

会话元数据及远程设备记录写回时保留未知字段；损坏的文件不能被空数据覆盖。配对审批请求与待领取令牌沿用旧版内存生命周期，不写入新的 `pairing-requests.json`。移除未接线的早期 `schedules.json`、`workflows.json`、`plugins.json`、`notification-settings.json`、`custom-ui.json`、`assets.json`、`memory.json` 存储实现，正式领域服务保持唯一所有者。

## 缓存与能力边界

路径核对不能代替功能验收。`pisper-session-label-index.json` 是旧 Node 的派生索引；Rust 从原 JSONL 标签条目构建内存读取缓存。`pisper-model-metadata.json` 和 `pi-dev-models-cache.json` 直接从原路径读取；后者沿用旧版长期参考数据的语义，按模型尾部 token 对齐取最大窗口，用户显式配置优先。读取和模型投影不改写这些文件；缺失或损坏时使用现有模型目录，不做目录迁移。

外部 Codex/Claude 凭据导入目前由 Rust 接口明确返回 `external_provider_import_not_supported`，并非从其他路径导入；它不影响读取已存入 Pisper 的 `auth.json`。这项既有功能缺口仍需单独补齐，不能宣称所有外部来源已获支持。旧领域内的计划格式升级、插件默认值版本标记与本次路径修复无关，不新增 Rust 数据目录迁移。

## 复现与验证

从 `v0.6.1` Windows 安装器提取的真实 `pisper-sidecar.exe`，在隔离环境下指向一个尚不存在的 agent 目录时以 `os error 2` 退出；预建相同目录后能完成握手。`v0.6.3` Runtime 分发包复现相同结果。原因是插件服务规范化根路径前，启动入口没有创建目录。这证明首次启动缺陷；仅凭 #34 的通用错误截图，不能断言它解释了报告者所有升级失败。

新增 `node scripts/smoke-rust-data-paths.mjs <runtime executable>` 使用临时目录与真实进程，覆盖目录不存在、自动令牌握手、bootstrap Cookie、健康检查、旧会话元数据、远程配对设置、重启与显式目录覆盖。已经接入启动闸门和桌面、Runtime、TUI 打包入口。存储单元测试验证未知字段保留、原路径写回、损坏文件保护与令牌不以明文落盘。

桌面/TUI 共用 Runtime 的源代码与协议验证可在 Windows 执行；macOS、Linux、Android、iOS 的实际构建及设备启动仍应由相应平台 CI/设备完成，不能以 Windows 测试代替。
## 本次验证记录（Windows x86_64 GNU）

- `cargo build --locked --manifest-path runtime-rs/Cargo.toml` 通过。
- Runtime 单元测试二进制串行复跑：504 通过、7 忽略；默认并行运行的 `native_post_exit_quiet_descendant_does_not_hold_output_forever` 清理断言曾失败，单独运行及整套串行复跑均通过。未跳过该测试，也未延长等待时间。
- `cargo test --locked --manifest-path src-tui/Cargo.toml`：162 通过。
- `npm run test:release`：92 通过、2 跳过。
- 按锁文件 `npm ci` 后，`npm run build` 和 `npm run check` 通过。后者包含 93 项真实 Runtime 可用性验收，以及新增数据路径、Provider 升级、项目上下文、会话缓存和 CLI 握手验证。
- 未运行其他操作系统构建、移动端设备验收或正式安装器重新打包；未创建新版本。
