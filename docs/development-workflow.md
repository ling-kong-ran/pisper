# 开发流程与验证

本文档记录 Pisper 的开发环境、依赖变更、构建复现和按影响范围选择验证的流程。规则分级与代码边界以根目录 `AGENTS.md` 为准；命令脚本和 CI 描述当前执行行为，已知偏差记录在文末，不能因文档更新而视为实现已完成。

## 环境与可复现性

- 源码开发和 Web/Runtime 验证以 Node.js 24 为基线，补丁版本必须满足 `package-lock.json` 中实际依赖的 `engines`。2026-09-18 审阅使用 Node 24.21.0；该版本是复现参考，不代表其他版本已通过验收。npm 版本以根目录 `package.json` 的 `packageManager` 字段为准，安装使用 `package-lock.json`，不引入第二套包管理器或锁文件。
- 分别核验宿主环境：桌面和独立 TUI 的 SEA 包自带 Node；移动端使用 `scripts/mobile-node-artifacts.json` 固定的嵌入式 Node；npm 启动器通过 `PISPER_RUNTIME_NODE` 使用当前宿主 Node 运行下载的 Runtime 闭包。启动器自身能够安装不代表完整 Runtime 可以在同一 Node 版本运行。
- 当前根清单和 `packages/pisper/package.json` 仍声明 Node ≥20，但锁定 Pi 依赖要求 ≥22.19，构建依赖另有补丁版本限制。这是待修正的兼容声明，不能据此宣称 Node 20 支持。调整最低版本时同步清单、生成的 npm 产物、文档和最低支持版本的 CI；只在 Node 24 上通过检查不能证明更低版本受支持。
- 构建、测试和发布脚本不得依赖开发者机器上的全局包、个人路径、未声明的环境变量或临时目录。需要特殊工具链时，在命令、脚本或对应平台文档中明确记录。
- 构建产物应能够追溯到源提交、工具版本、构建目标和关键输入。发现两个环境产物不一致时，先记录这些信息，再判断是源码、依赖、工具链还是环境差异。
- Android、iOS、Tauri 和 TUI 的额外环境要求分别见 `docs/mobile.md`、`docs/node-sea-webview.md` 和仓库中的平台脚本。缺少设备、签名、Xcode 或 Rust 工具链时，报告未验证项，不要把交由其他人执行的步骤写成已通过。

## 依赖和生成产物

- 新增或升级 npm、Rust、原生、模型和二进制依赖前，记录用途、来源、版本、许可证、维护状态、安全风险、体积影响和平台兼容性。
- 升级依赖后至少检查锁文件、类型检查、相关测试、构建产物、许可证、安全报告和 Node/Rust/原生工具链要求。检查应分别说明源码宿主、打包宿主与用户运行环境。安全修复或会改变发布产物的依赖更新按实际影响判断是否需要发布。
- Pi 0.86 的 `agent.state.systemPrompt` 是历史消息派生的只读值。Pisper 通过 `before_agent_start` 注入每轮身份和运行时约束，缓存诊断读取当前 `session.systemPrompt` 的投影，不改写历史 system 消息。提示词测试同时覆盖旧可写状态和新只读状态；内置 Provider 测试验证当前目录中的型号能保存和解析，不把上游某个历史型号当作永久契约。
- Pi 0.86 的剪贴板与终端原生辅助模块统一位于 `pi-tui/native/<platform>/prebuilds/`；SEA 按目标平台保留并审计 `*-platform.node`（Linux 为 `linux-platform-x11.node`），移动端仍移除桌面绑定。更新 Pi 后须运行 `sidecar:sea` 和 `sidecar:sea:smoke`，验证动态原生依赖的实际加载，不能仅凭源码测试判断打包兼容。
- `src/vendor/`、移动端生成工程、补丁产物和发布暂存目录必须通过对应脚本维护。修改生成源或脚本后重新生成，不要直接编辑下次构建会覆盖的结果。
- 资源、模型和二进制文件应记录来源、版本、校验摘要和打包路径；不得把临时副本或个人测试数据放入源码和发布目录。

## 按影响范围验证

| 改动范围 | 最低验证 | 需要补充的情况 |
| --- | --- | --- |
| 纯文档与规范 | `git diff --check`、本地链接、命令引用与规则一致性 | 文档被源码守卫读取或改变可执行契约时运行对应测试；不默认执行全量构建 |
| TypeScript 或界面 | 受影响范围的类型检查、lint、i18n 检查 | 涉及多个前端区域或构建配置时运行 `npm run check` |
| 文件移动、拆分、合并或公共入口 | 检查调用方、依赖方向、状态所有权、导出、路由、测试发现和受影响守卫；运行对应类型检查与行为测试 | 影响前端入口、懒加载或重型依赖时运行 `npm run build`；跨客户端按协议及平台行补充 |
| Runtime 工具、服务或路由 | 相关 `runtime/tests/` 测试 | 修改公共 HTTP/SSE 输出时运行 `npm test`、`npm run tui:check` 和 `npm run tui:test` |
| TUI Rust | `cargo fmt --manifest-path src-tui/Cargo.toml -- --check`、`npm run tui:check`、`npm run tui:test`、`cargo clippy --manifest-path src-tui/Cargo.toml --all-targets -- -D warnings` | 涉及帮助文本、终端交互或协议时补充对应覆盖测试 |
| Tauri 桌面端 | 在 `src-tauri/` 下运行 `cargo fmt -- --check`、`cargo check`、`cargo test` 和 `cargo clippy --all-targets -- -D warnings` | 涉及打包、权限或更新器时运行对应冒烟测试和平台构建 |
| SEA 或桌面打包 | `npm run sidecar:sea:smoke` | 目标涉及平台安装包时执行平台专属打包 |
| npm 安装器或 npm 发布 | `npm run npm:pack:check`、`node scripts/validate-npm-targets.mjs`、`runtime/tests/npm-cli-package.test.mjs` | 涉及发布渠道时遵循发布脚本和工作流的额外检查 |
| Android 发布回归 | 启用 R8 的真实压缩发布变体 | 在临时 Android 用户/配置文件中安装并操作，避免影响主用户数据 |
| iOS 原生构建或发布 | 遵循 `docs/mobile.md` 的设备/模拟器流程 | Apple Silicon 使用原生 arm64 Node，区分设备和模拟器的 `libapp.a`，并实际启动模拟器 |

无关平台不要求执行完整构建。进入合并或发布阶段时，主代理或负责合并的人应运行适用范围的完整检查，并在变更说明中列出未运行项目及原因。

## 检查范围与结果解释

- `npm run typecheck` 覆盖 `src/`、Vite 配置和 `tsconfig.jscheck.json` 的指定文件及其依赖图，不等于全部 Runtime 已检查。当前 JS 配置列出 `runtime/http/route-registry.mjs`、`runtime/services/decision-remote-client.mjs`、`runtime/services/decision-service.mjs`、`runtime/runtime/workspace-directories.mjs`、`runtime/services/openai-request-transport.mjs` 和 `shared/app-update.mjs`，并检查它们引入的依赖（包括决策领域契约、模型注册表、Jev SDK 适配与答案校验）；扩大覆盖时记录实际纳入的范围，避免用宽泛 `any` 消除错误。
- `npm test` 当前只匹配 `runtime/tests/*.test.mjs`，其中包含 Runtime、前端纯逻辑、协议、构建和源码守卫。新增其他目录或嵌套目录时，同步执行入口、CI 和测试资产路径，验证默认命令确实发现新测试；不要求为了目录整齐搬迁现有测试。
- `npm run check` 按顺序执行类型、lint、i18n、格式和启动检查；任一步失败会阻断后续步骤。`npm run build` 中编译、体积预算和语法兼容检查也按顺序执行。报告应区分已通过、失败、跳过和未执行，不能用前一步成功概括整条命令通过。
- 源码守卫应保护依赖边界、安全、兼容性和构建约定。重构前用 `node scripts/list-source-guards.mjs --source <文件>` 辅助定位，再检索动态路径或间接读取；该脚本不是完整静态分析器。更新守卫时记录原保护目标和替代的行为、契约或依赖检查。
- 行数不是架构正确性的判据；现有行数硬门槛按下方台账迁移。禁止靠删除空行、搬移共享 `this` 方法、任意上调阈值或跳过测试消除失败。体积预算与行数不同，它约束交付产物；预算变更需要测量依据、场景和影响说明，不能仅因超限放宽。
- 失败时先区分实现回归、环境不符、时序不稳定和过时断言。只有新修改、失败诊断或未解决的疑点才需要重复验证；同一命令连续失败两次且无进展时停止盲目重试并说明下一步。已有失败同样如实报告，不能记为本次通过。

## 依赖、测试和评审记录

- 测试使用临时目录和模拟服务，不依赖真实凭据、开发者个人目录或外部网络。结束时先停止服务并等待持久化队列、后台任务和进程退出，再删除目录、释放端口与恢复环境。`ENOTEMPTY` 等清理失败应检查未收尾任务，不通过吞掉异常或任意延时掩盖。
- 代码评审说明应包含目的、影响范围、验证结果、已知风险、迁移步骤和回滚方式。跨层、跨客户端、公共协议、存储格式和平台桥接改动需要邀请相关维护者审查。
- 用户可见行为、命令、配置、协议、存储格式和目录边界变化时，同步更新测试、帮助文本、文档和必要的发布说明。不要把无关的全仓库格式化或重命名混入变更。

## 已知迁移项

以下为 2026-09-18 源码审阅确认的偏差，不是豁免，也不表示对应修复已经实施。推进该领域变更的负责人承接相应项，按业务边界分批修复；关闭时记录验证依据并更新本表，新增问题同样登记范围与完成条件。

| 责任范围 | 当前偏差 | 完成条件 |
| --- | --- | --- |
| 环境与打包 | 根清单、npm 启动器的 Node ≥20 声明低于当前 Runtime 依赖要求 | 分别核验各宿主，统一清单、生成产物与文档，并验证声明的最低支持版本 |
| 架构守卫 | 聊天行数断言已由编排检查和真实导入图检查替代，Runtime/路由已有依赖与动态导入检查 | 新增领域沿用依赖/行为检查；动态计算路径仍需人工核对，详见[客户端边界记录](architecture/frontend-boundaries.md) |
| Runtime 类型与公共协议 | JS 类型检查只覆盖少量文件，`src/types/chat.ts` 仍有宽泛 `EntityRecord` | 按领域补充 JSDoc/checkJs、边界校验、明确字段及 Web/TUI 契约测试，未知扩展限制在独立区域 |
| HTTP 与错误契约 | JSON/文本解析已分离，JSON/SSE 共用 HTTP 错误解析，MCP 使用字段校验；其他旧泛型调用尚未普遍校验字段，Web/TUI 仍匹配部分错误文案 | 逐域迁移 unknown 解码；以兼容方式补齐稳定错误码及旧客户端契约 |
| 前端请求与生命周期 | Memory/Schedules 等页面自行管理请求；部分 Runtime 测试有异步清理错误 | 统一数据所有权、取消与去重，覆盖乱序响应；测试先等待服务及写入任务收尾再删除目录 |
| Feature 归属与入口 | MCP 已归独立领域并使用查询缓存，聊天工具栏 Store 已归聊天，当前跨 Feature 导入已有公开契约与守卫；其他领域仍需核验入口和全局模型归属 | 按领域继续治理，避免无选择的聚合导出，保持懒加载和依赖检查 |
| Runtime/TUI 职责 | 核心 Runtime 仍通过继承、原型注入共享大量状态；TUI 混合多个渲染与交互职责 | 分批明确状态所有者和窄接口；按独立变化原因拆分或合并，保持行为与跨端契约，不能仅以减少行数验收 |

### 决策审批与 Computer Use 验证接线

决策服务仍由 App Runtime 持有，通过 `SkillsService` 的窄依赖传入资源加载器。`runtime/runtime/computer-use-verification.mjs` 只装饰 Pi 已加载的官方 `act_ui` 定义，复用其参数、执行闭包与会话生命周期；不再单独加载同名扩展或创建第二份 bridge 状态。主会话及 Runtime 派生的子代理共用此接线，未注入决策服务的独立加载器保留官方工具。

审批参数先经过现有结构化凭据检测；命中时不外发、不对脱敏后的不完整输入自动批准，而是回落人工审批。检测复用 `runtime/security/secret-redaction.mjs` 的模式，属于保守的已知凭据检测，不能保证识别所有无标签秘密。验证入口为 `runtime/tests/computer-use-verification.test.mjs`、`runtime/tests/decision-service.test.mjs` 和既有脱敏测试。回滚应同时恢复加载器及工具装饰接线，不能恢复两份 `bridge.ts` 实例；无持久化迁移。

### 决策模型扩展

决策服务通过静态注册表选择供应商、协议和型号能力，新增协议由窄适配器实现。自动审批策略按精确型号登记，阈值绑定供应商、型号、端点和策略版本；无策略或绑定不匹配时回落会话权限。现有 Jev 默认型号采用兼容策略，不代表新模型自动获得同样的概率解释。迁移、回滚及三端范围见[决策模型边界](architecture/decision-models.md)，测试入口为 `runtime/tests/decision-models.test.mjs` 及原决策协议测试。
