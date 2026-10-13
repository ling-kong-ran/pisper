# 开发流程与验证

本文档记录 Pisper 的开发环境、依赖变更、构建复现和按影响范围选择验证的流程。规则分级与代码边界以根目录 `AGENTS.md` 为准；命令脚本和 CI 描述当前执行行为，已知偏差记录在文末，不能因文档更新而视为实现已完成。

## 环境与可复现性

- 源码开发和 Web/Runtime 验证以 Node.js 24 为基线，补丁版本必须满足 `package-lock.json` 中实际依赖的 `engines`。2026-09-18 审阅使用 Node 24.21.0；该版本是复现参考，不代表其他版本已通过验收。npm 版本以根目录 `package.json` 的 `packageManager` 字段为准，安装使用 `package-lock.json`，不引入第二套包管理器或锁文件。
- 当前 Rust 布局的桌面与 CLI 交付原生 Runtime，移动端静态链接同一 Rust 库；Node 24 用于构建和 npm 启动器。历史 Node 分发仍按旧 backend 元数据处理，不可与新 Runtime_Rust 归档混用。
- 当前根清单和 `packages/pisper/package.json` 声明 Node ≥22.19，但源码构建仍按 Node 24 验证，构建依赖另有补丁版本限制。不能据此宣称 Node 20 支持。调整最低版本时同步清单、生成的 npm 产物、文档和最低支持版本的 CI；只在 Node 24 上通过检查不能证明更低版本受支持。
- 构建、测试和发布脚本不得依赖开发者机器上的全局包、个人路径、未声明的环境变量或临时目录。需要特殊工具链时，在命令、脚本或对应平台文档中明确记录。
- 构建产物应能够追溯到源提交、工具版本、构建目标和关键输入。发现两个环境产物不一致时，先记录这些信息，再判断是源码、依赖、工具链还是环境差异。
- Android、iOS、Tauri 和 TUI 的额外环境要求分别见 `docs/mobile.md` 和仓库中的平台脚本；`docs/node-sea-webview.md` 仅保留 0.5.x 历史 SEA 布局说明。缺少设备、签名、Xcode 或 Rust 工具链时，报告未验证项，不要把交由其他人执行的步骤写成已通过。

## 依赖和生成产物

- 新增或升级 npm、Rust、原生、模型和二进制依赖前，记录用途、来源、版本、许可证、维护状态、安全风险、体积影响和平台兼容性。
- 升级依赖后至少检查锁文件、类型检查、相关测试、构建产物、许可证、安全报告和 Node/Rust/原生工具链要求。检查应分别说明源码宿主、打包宿主与用户运行环境。安全修复或会改变发布产物的依赖更新按实际影响判断是否需要发布。
- Pi 引擎为固定版本的 Rust pi-rs（`runtime-rs/vendor/pi-rs/`），来源与补丁分别记录在 `runtime-rs/pi-rs-source.json` 和 `runtime-rs/vendor/pi-rs/PISPER-PATCHES.md`。升级引擎使用 `npm run pi:rust:update` 并同步上述记录；升级后须运行 `npm test`、`npm run check` 的启动闸门及相关冒烟，验证工具、MCP 与原生依赖的实际加载，不能仅凭单元测试判断打包兼容。内置 Provider 测试验证当前目录中的型号能保存和解析，不把上游某个历史型号当作永久契约。
- 语音等原生资源经 `scripts/stage-rust-speech.mjs` 校验后暂存进发布闭包；改动相关依赖或暂存脚本后须运行对应冒烟验证实际加载。Node SEA 打包流程已随 0.5.x 移除，不再适用。
- `src/vendor/`、移动端生成工程、补丁产物和发布暂存目录必须通过对应脚本维护。修改生成源或脚本后重新生成，不要直接编辑下次构建会覆盖的结果。
- 资源、模型和二进制文件应记录来源、版本、校验摘要和打包路径；不得把临时副本或个人测试数据放入源码和发布目录。

## 按影响范围验证

| 改动范围 | 最低验证 | 需要补充的情况 |
| --- | --- | --- |
| 纯文档与规范 | `git diff --check`、本地链接、命令引用与规则一致性 | 文档被源码守卫读取或改变可执行契约时运行对应测试；不默认执行全量构建 |
| TypeScript 或界面 | 受影响范围的类型检查、lint、i18n 检查 | 涉及多个前端区域或构建配置时运行 `npm run check` |
| 文件移动、拆分、合并或公共入口 | 检查调用方、依赖方向、状态所有权、导出、路由、测试发现和受影响守卫；运行对应类型检查与行为测试 | 影响前端入口、懒加载或重型依赖时运行 `npm run build`；跨客户端按协议及平台行补充 |
| Runtime 工具、服务或路由 | `npm test` 中相关的 Rust 测试（可用 `cargo test <名称过滤> --manifest-path runtime-rs/Cargo.toml` 缩小范围） | 修改公共 HTTP/SSE 输出时运行 `npm test`、`npm run tui:check` 和 `npm run tui:test` |
| TUI Rust | `cargo fmt --manifest-path src-tui/Cargo.toml -- --check`、`npm run tui:check`、`npm run tui:test`、`cargo clippy --manifest-path src-tui/Cargo.toml --all-targets -- -D warnings` | 涉及帮助文本、终端交互或协议时补充对应覆盖测试 |
| Tauri 桌面端 | 在 `src-tauri/` 下运行 `cargo fmt -- --check`、`cargo check`、`cargo test` 和 `cargo clippy --all-targets -- -D warnings` | 涉及打包、权限或更新器时运行对应冒烟测试和平台构建 |
| 桌面打包 | `npm run desktop:webview:package` 及对应冒烟脚本 | 目标涉及平台安装包时执行平台专属打包 |
| npm 安装器或 npm 发布 | `npm run npm:pack:check`、`node scripts/validate-npm-targets.mjs` | 涉及发布渠道时遵循发布脚本和工作流的额外检查 |
| Android 发布回归 | 启用 R8 的真实压缩发布变体 | 在临时 Android 用户/配置文件中安装并操作，避免影响主用户数据 |
| iOS 原生构建或发布 | 遵循 `docs/mobile.md` 的设备/模拟器流程 | Apple Silicon 使用原生 arm64 Node，区分设备和模拟器的 `libapp.a`，并实际启动模拟器 |

无关平台不要求执行完整构建。进入合并或发布阶段时，主代理或负责合并的人应运行适用范围的完整检查，并在变更说明中列出未运行项目及原因。

工作流的文字/图片/视频输入、ZIP 依赖包与精灵图算法使用说明见[工作流文档](workflows.md)，职责与回滚边界见[工作流输入与精灵图架构](architecture/workflow-inputs-and-sprites.md)。`npm run test:workflows:ui` 提供隔离 Runtime 的浏览器验收，也由 `npm run test:ui` 执行；真实本地算法验收需按文档准备固定版本测试资源。

## 检查范围与结果解释

- `npm run typecheck` 覆盖 `src/`、Vite 配置和 `tsconfig.jscheck.json` 的指定文件及其依赖图，不等于全部共享层已检查。当前 JS 配置只列出 `shared/*.mjs` 共享模块，并检查它们引入的依赖；扩大覆盖时记录实际纳入的范围，避免用宽泛 `any` 消除错误。
- `npm test` 运行 `runtime-rs/` 的全部 Rust 测试；发布、打包与源码守卫的 Node 测试位于 `scripts/tests/`，经 `npm run test:release` 执行；UI 冒烟经 `npm run test:ui*` 脚本执行。新增测试目录或嵌套位置时，同步执行入口、CI 和测试资产路径，验证默认命令确实发现新测试；不要求为了目录整齐搬迁现有测试。
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
| 环境与打包 | 清单已声明 Node ≥22.19；源码构建目前按 Node 24 验证，最低声明版本仍需独立验收 | 分别核验构建宿主与 npm 启动器，统一清单、生成产物与文档，并验证声明的最低支持版本 |
| 架构守卫 | 聊天行数断言已由编排检查和真实导入图检查替代，Runtime/路由已有依赖与动态导入检查 | 新增领域沿用依赖/行为检查；动态计算路径仍需人工核对，详见[客户端边界记录](architecture/frontend-boundaries.md) |
| Runtime 类型与公共协议 | JS 类型检查（`tsconfig.jscheck.json`）当前只覆盖 `shared/*.mjs` 共享模块（2026-10-13 核实）；`src/types/chat.ts` 仍有宽泛 `EntityRecord`，但已注释标注为流式协议扩展字段的逃生口 | 按领域补充边界校验、明确字段及 Web/TUI 契约测试，未知扩展限制在独立区域；`EntityRecord` 的移除以流式协议字段完成显式建模为条件 |
| HTTP 与错误契约 | JSON/文本解析已分离，JSON/SSE 共用 HTTP 错误解析，MCP 使用字段校验；其他旧泛型调用尚未普遍校验字段，Web/TUI 仍匹配部分错误文案 | 逐域迁移 unknown 解码；以兼容方式补齐稳定错误码及旧客户端契约 |
| 前端请求与生命周期 | Memory/Schedules 页面仍自行管理请求（2026-10-13 核实：MemoryPage 直接使用 apiJson，未接入 React Query）；原 Node Runtime 测试的异步清理问题随 Rust 迁移失效 | 统一数据所有权、取消与去重，覆盖乱序响应 |
| Feature 归属与入口 | MCP 已归独立领域并使用查询缓存，聊天工具栏 Store 已归聊天，当前跨 Feature 导入已有公开契约与守卫；其他领域仍需核验入口和全局模型归属 | 按领域继续治理，避免无选择的聚合导出，保持懒加载和依赖检查 |
| Runtime/TUI 职责 | 原 Node Runtime 的继承/原型注入问题随 Rust 迁移消除；新现状是 `runtime-rs/src/lib.rs` 的 `AppState` 仍集中 53 个字段（2026-10-13 核实），与"不把整个 AppState 作为通用依赖传递"的规范相悖；TUI 混合多个渲染与交互职责 | 按领域把 AppState 字段迁入 WorkflowServices 一类的子结构或领域模块；TUI 分批明确状态所有者和窄接口，保持行为与跨端契约，不能仅以减少行数验收 |

### npm 依赖安全修复与钉版记录（2026-10-13）

本轮以 `npm audit fix` 加定点升级处理安全债务，26 个漏洞降至 12 个：critical `proxy-addr` 与 `smol-toml` 解析拒绝服务已修复；生产依赖 `officeparser` 由 7.8.0 升至 ^8.1.1 以修复其 `pdfjs-dist` 高危（officeparser 仅被 legacy SEA 脚本引用，主版本升级不影响现行 Rust 流程）。

连带钉版：`streamdown` 2.5.0 与 `@streamdown/math` 1.0.2 钉为精确版本。2.7.0 的 `ThemeInput` 类型放宽后与 shiki 高亮插件接口不兼容（`src/lib/streaming/streamdown.ts:80/146` 报 TS2322），而本次升级不含安全修复。钉版使 streamdown/mermaid 两个无修复低危项保留在审计报告中，属于已接受的取舍。解除钉版的条件：为 streamdown ≥2.7 的 ThemeInput 增加显式收窄转换并通过 typecheck 与构建预算检查。

暂不修复项及原因：shadcn CLI 链（braces/micromatch/fast-glob/ts-morph，high）只在开发期组件脚手架使用，不随产物分发，npm audit 建议的"修复"指向 shadcn@1.0.0 属于包谱系回退，不予采用，完成条件为 shadcn 当前发布线给出真实修复版本；katex/remark-math/micromark-extension-math/mermaid（low）上游无可用修复。CI 中 `npm audit` 仍为非阻断。

`@earendil-works/pi-coding-agent` npm 依赖仍保留：npm 安装器对旧 Node 后端的兼容路径（`packages/pisper/bin/pisper.mjs` 的 `runtimeBackend !== 'rust'` 分支）与 `scripts/release.mjs` 的自动升级步骤仍在使用；移除条件为旧 Node 后端兼容路径正式下线。

### runtime-rs CI 质量门禁记录（2026-10-13）

AGENTS.md 要求 Rust 提交前运行 `cargo fmt` 与 `clippy`，但此前 CI 只对 src-tui 强制。已为 `runtime-rs` 增加非阻断（`continue-on-error`）的 fmt/clippy job，范围限定 `-p pisper-server` 以排除 vendored pi-rs 上游源码。本机无 Rust 工具链，基线是否零告警未验证；升级为阻断门禁的条件：在有工具链的环境运行 `cargo fmt --manifest-path runtime-rs/Cargo.toml -p pisper-server -- --check` 与 `cargo clippy --locked --manifest-path runtime-rs/Cargo.toml -p pisper-server --all-targets -- -D warnings` 全绿后，移除 `continue-on-error`。

### 决策审批与 Computer Use 验证接线

> 本节描述 0.5.x Node Runtime 的接线方式，其中 `runtime/` 路径与测试入口已随 Rust 迁移失效；当前实现位于 `runtime-rs/src/native_decisions.rs`（含同文件 `mod tests`），能力状态以 `docs/reports/rust-release-parity*.json` 为准。

决策服务仍由 App Runtime 持有，通过 `SkillsService` 的窄依赖传入资源加载器。`runtime/runtime/computer-use-verification.mjs` 只装饰 Pi 已加载的官方 `act_ui` 定义，复用其参数、执行闭包与会话生命周期；不再单独加载同名扩展或创建第二份 bridge 状态。主会话及 Runtime 派生的子代理共用此接线，未注入决策服务的独立加载器保留官方工具。

审批参数先经过现有结构化凭据检测；命中时不外发、不对脱敏后的不完整输入自动批准，而是回落人工审批。检测复用 `runtime/security/secret-redaction.mjs` 的模式，属于保守的已知凭据检测，不能保证识别所有无标签秘密。验证入口为 `runtime/tests/computer-use-verification.test.mjs`、`runtime/tests/decision-service.test.mjs` 和既有脱敏测试。回滚应同时恢复加载器及工具装饰接线，不能恢复两份 `bridge.ts` 实例；无持久化迁移。

### 决策模型扩展

> 本节记录 Node 决策服务的设计；当前原生移植位于 `runtime-rs/src/native_decisions.rs`，测试入口为 `npm test`（`cargo test`）中的对应模块测试。

决策服务通过静态注册表选择供应商、协议和型号能力，新增协议由窄适配器实现。自动审批策略按精确型号登记，阈值绑定供应商、型号、端点和策略版本；无策略或绑定不匹配时回落会话权限。现有 Jev 默认型号采用兼容策略，不代表新模型自动获得同样的概率解释。迁移、回滚及三端范围见[决策模型边界](architecture/decision-models.md)，测试入口为 `runtime/tests/decision-models.test.mjs` 及原决策协议测试。

### macOS 原生浏览器验收脚本迁移项（2026-10-08）

移动三屏修复验收时，`npm run check` 的类型、lint、i18n 和格式阶段通过，启动闸门在 `scripts/smoke-rust-browser.mjs` 的进程归属检查失败：`backendPid`、`processRows` 和 `assertGone` 无条件调用 `powershell.exe`。该脚本与 Rust 后端相对当时 release 基线 `60cf5548` 未变，不能把该失败记为移动界面回归，也不能记为完整检查通过。macOS 的 Chrome Dev 需以实际 App 可执行文件启动，直接软链接启动器可能无法定位 Framework；配置可用浏览器后仍会遇到上述 Windows 命令限制。

责任范围为 Runtime 浏览器验收脚本。迁移完成条件：在 macOS/Linux 增加等价的监听端口归属、浏览器后代进程身份（含防 PID 复用）和退出回收检查，保留 Windows 验证，并实际通过原生浏览器与重启验收；不得跳过该用例或删除进程清理断言。当前移动端独立验证入口为 `npm run test:mobile-shell`，它不替代原生浏览器验收。

同次 macOS Rust 单元测试需让 `TMPDIR` 指向不含符号链接的真实临时目录；默认 `/var` 或 `/tmp` 别名会触发既有存储路径拒绝策略。真实临时目录下 `npm test` 为 458 项通过、6 项忽略；不要通过放宽存储安全检查解决测试路径问题。

### Rust 布局发布流程（2026-10-09）

当前源码包含 `runtime-rs/Cargo.toml`，已移除 Node Runtime 入口。统一入口仍为 `npm run release -- <patch|minor|major|X.Y.Z>`，按各渠道标签和实质性变更检测 Desktop、TUI、Rust Runtime、npm 与移动 App。依次发布 TUI、Runtime、Desktop、App；Runtime/TUI 改动会链式更新安装包和 npm。Rust 布局不再自动升级旧 Pi Node 依赖。Windows x64、Mac ARM/Intel、Linux x64 共用平台表与原生资源暂存。
发布前先确认本地 `release` 与 `origin/release` 同步、tracked 工作区干净、目标版本大于现有版本且标签未创建。入口运行既有质量检查后，派发 `.github/workflows/release.yml` 并等待结果；该工作流在资产验证完成后原子提交版本和标签、上传并公开 GitHub Release。运行期间冻结 `release`，失败或需要修正候选时先检查运行状态、日志与远端分支，确认原运行终止后再集成修正并从统一入口重试；不要手动改版本或创建标签。

`npm run test:release` 在隔离临时布局中运行真实发布入口，拦截外部命令以验证 Rust/Node 渠道选择、版本与分支拒绝条件，不执行安装、Git 写入、构建或远端发布；发布质量阶段运行同一测试。Windows Desktop 构建前还运行 `npm run test:project-directory` 验证项目目录交互。桌面资产清单恢复四平台安装包、Windows 离线包、签名、四平台前端组件和完整 `latest.json`。独立 Runtime 使用 `Runtime_Rust` 资产名；npm 发布源显式记录 backend，旧 Node 元数据仍按旧布局读取。新更新器拒绝激活旧 Node Runtime。

Desktop 构建在 `npm ci` 后通过当前锁定的 `playwright-core` CLI 安装 Chromium，并验证普通 Chromium 和 headless shell 均可启动，再执行目录交互与长时间 Rust 打包检查。桌面构建失败时，各平台仅上传隔离测试报告、服务日志与失败截图，诊断资产不参与发布清单。长构建可设置 `PISPER_RELEASE_POLL_SECONDS=600`，使统一发布入口每十分钟检查当前渠道；未设置时保留 GitHub CLI 的默认间隔，任何渠道失败仍停止后续派发。

### Rust 会话、CLI 与标记验收补充（2026-10-11）

当前 Rust 源码的独立 CLI 本地构建入口为 `npm run tui:build`（等同 `tui:package`），不再调用已删除的 `sidecar:sea`。四个平台共用桌面的语音、rg/fd 与许可证暂存函数，同时包含 Web 前端；发布渠道已接入相应原生包。Mac ARM 最低 14.0；Intel 受锁定 ONNX Runtime 二进制约束，最低 15.5，不能将其声明为兼容更旧系统。开发 CLI 需先构建 `runtime-rs`；具体命令见 `src-tui/README.md`。此前文档中的 SEA 流程只适用于仍保留 Node Runtime 的旧布局。

启动闸门新增 `scripts/smoke-rust-cli.mjs`，实际构建并启动 CLI，使用隔离 HOME 检查 doctor、工作目录、重启与错误路径。`scripts/smoke-rust-session-read-cache.mjs` 覆盖大日志分页、标记、并发元数据修改、原始 `.pisper` 项目命令发现及 Skill 开关并发写入。`npm run test:mobile-shell` 还覆盖终端目录等待/失败/取消，以及会话工作流慢请求去重、切换取消与迟到响应隔离。会话读取的测量方法、前后数据和回滚范围见[会话读取缓存](architecture/session-read-cache.md)。

本轮 Runtime `cargo clippy --all-targets` 能完成，但存量源码与供应商依赖仍有大量警告；`cargo fmt --check` 也发现原有跨模块格式偏差。不能把“能构建”描述为 Runtime 全仓库零警告或格式闸门通过。修复范围内的新文件使用 rustfmt；不要通过一次全仓库格式化掩盖这批性能与生命周期修复。TUI 单独运行其格式、测试和 `clippy --all-targets -- -D warnings`。实际原生验收平台为 macOS arm64，Windows/Android/iOS 的设备运行不能由本机测试代替。

浏览器原生验收现通过 `scripts/native-process-inspection.mjs` 在 POSIX 主机使用 `ps`/`lsof` 检查监听端口归属、浏览器后代 PID 与启动时间，保留 Windows CIM 路径。单元测试覆盖应用路径含空格、后代范围、PID 复用和进程换名；实际浏览器测试仍要求主机安装可发现的 Chrome/Edge/Chromium。macOS 本轮使用现有 Chrome Dev 的真实可执行文件，并以测试专用 PATH 启动器接入发现逻辑，不访问用户浏览器资料目录。浏览器夹具清理仅恢复它拥有的能力开关，保留后续插件重启夹具安装的工具，避免整份旧快照覆盖其他夹具状态。

### 移动三视图手势与翻页（2026-10-11）

`MobileThreePane.tsx` 保留三屏内容与导航编排，原触摸监听移入布局层 `useMobilePaneTransition.ts`。Hook 只接收手机布局是否启用、当前视图与切换回调；拖动位置、动画句柄、监听器与取消均为其本地生命周期状态，不拥有 Feature 业务状态或新增持久化键。三屏保持挂载，平板三栏不播放翻页。手机支持单指横滑、短滑回弹、左右图标按钮和键盘导航；纵向滚动、输入框、横向滚动内容和显式手势区域优先处理自身交互。

翻转变换仅在拖动与 220 ms 过渡期间存在，结束、取消、布局变化与卸载时清理，避免长期改变内部 fixed 弹窗的定位参照。系统和应用的减少动态效果设置均生效。工作区标签单独横向滚动，“文件”操作留在右侧非滚动区域。

该改动只涉及共用 Web 布局，没有新增桌面、Android、iOS 原生命令、权限或资源。验收入口仍为 `npm run test:mobile-shell`，覆盖跟手变换、双向导航、边界、取消、多指、输入/滚动保护、动画清理、减少动态效果、图标按钮、文件操作定位及原有面板生命周期；配合类型、lint、i18n 与生产构建检查。浏览器触摸模拟不等于 Android/iOS 真机验收。回滚时一起恢复布局组件、Hook 调用和测试，不需要迁移用户数据。

三视图点击响应补充：导航在点击事件中立即启动动画，再用 React transition 更新父级视图状态；动画帧只修改 transform/opacity，固定位置只在起止时更新。跨两屏直接翻转首尾页，不让中间聊天页经过视口。完成与取消均释放变换和合成层提示。聊天 Dock 只接收真正变化的上下文状态，切屏不再重建整组聊天操作回调；附件消费回调也保持稳定。

隔离压力场景为 Apple M1 / 16 GiB / macOS 26.6.2，Chromium 151，390×844 视口，两个面板共 2400 行静态内容，4 倍 CPU 降速，工作区与会话列表往返六次。点击到下一帧的中位时间从 83.9 ms 降到 28.3 ms，布局次数中位值从 18 降到 4；这不是用户真机延迟承诺。设置 `PISPER_MOBILE_PERF_REPORT=<绝对 JSON 路径>` 后运行 `npm run test:mobile-shell` 可生成同场景记录，再继续运行行为验收。回归保护检查六个切换方向在通知父组件之前已启动动画、帧间不动画布局属性、父状态延后时连续点击/反向请求不会丢失，不绑定机器相关时间阈值。

### 桌面多会话分屏恢复（2026-10-11）

聊天分屏入口在 2026-09-27 的界面改版提交 `6cb2967d` 中关闭，早于 Rust Runtime 迁移；v0.5.80 和 v0.5.81 已使用单会话页面。恢复复用现有 Dockview 和原 `pisper-chat-dock-layout.json`，不迁移配置、不增加独立系统窗口。桌面会话标签支持拖动和四向拆分，标签右键提供关闭、移动和拆分操作，聚焦标签后可用 Shift+F10 或菜单键打开同一菜单，历史页可以直接把另一会话打开到指定方向。桌面工作目录入口位于输入框上方，调用既有系统目录选择器；辅助页“+”紧邻页签。桌面会话工具、窗口按钮与标签共用一行：辅助栏关闭时两组按钮相邻，打开时窗口按钮进入辅助栏右侧，会话工具留在主栏最上方、最右侧分组。常规宽度下按钮参与正常布局、标签可滚动；分组最小宽度总和超过主区域时，按钮固定到主区域可见右沿，避免窗口控制被溢出分组裁掉。非聊天页面仍使用独立标题栏。浏览器入口位于各分组标签行。手机与移动 App 继续单会话及三视图布局。桌面内嵌窗口同样支持分屏，不再使用额外的900px断点隐藏操作。

布局状态仍由 `useChatDock` 拥有，分屏可用性跟随单会话布局与统一的手机断点；没有跨 Feature 依赖变化。Dockview 销毁之前保存、解绑，手机选择在回到桌面后重放。读取失败或格式不可识别时使用临时界面并提示重入重试，禁止用默认布局覆盖旧文件；关闭面板继续尊重流式会话所有权。构建守卫原先禁止打包任何 Dockview，现在改为要求桌面入口懒加载，并禁止该 vendor 进入应用及手机聊天的静态依赖图，保留既有体积与语法兼容预算。

验收入口为 `npm run test:chat-dock`，先运行 `cargo build --locked --manifest-path runtime-rs/Cargo.toml`、`npm run build`，并安装 Playwright Chromium。浏览器组件测试覆盖四方向、关闭/删除、刷新、移动往返、损坏布局与网络失败、加载期间选择、键盘和流式状态释放契约；真实 Rust HTTP 测试覆盖旧路径读写、非法请求、并发原子保存和重启，再使用真实生产 ChatPage 验证分屏、独立草稿、历史入口与手机不加载 Dockview。CI 在生产构建之后执行同一入口。共用 Web 层改动不涉及桌面/Android/iOS 原生命令、权限或资源；本机浏览器验收不代表原生系统窗口或移动真机验收。回滚需同时恢复页面、Hook、懒加载守卫与测试入口，原布局文件保持兼容。

窄桌面补充：会话面板使用 Dockview 默认的100px最小尺寸，恢复旧布局前按当前约束解析，避免旧300px宽度下限把左右分屏撑出容器。输入工具区按真实测量宽度收纳到“+”；`useSessionViewport` 持有面板高度观察器，低于560px时空会话省略欢迎装饰与快捷建议，让输入区直接可见。实际消息继续显示，高度跨过阈值才触发React状态变化，卸载释放观察器。新增825×712视口、含264px侧栏的四向边界、旧布局恢复、独立草稿、工具入口和输入区边界回归；本机内嵌页面另验收上下分屏。拆分或拖动完成后立即保存结构，连续缩放仍防抖；恢复测试等待保存响应中的分组结构，不能仅凭已打开的会话ID认定分屏已落盘。

分组“+”新建补充：按点击时的分组ID和工作目录创建，POST成功后直接打开该组页签，不再等待全量目录刷新；共享查询先合并服务端创建结果，再由已有失效机制后台校准。同一分组去重在途请求，跨分组请求保持独立，按钮等待期间禁用并显示进度。目标分组在响应前已关闭时回落到当前组，不持有已销毁的分组对象。真实生产回归使用不同工作目录，并延迟真实POST响应和列表GET，覆盖目录失败、乱序响应、跨组并发、切换焦点及关闭目标组；入口仍为 `npm run test:chat-dock`。

### UI/UX 错误恢复与功能入口审查（2026-10-11）

本轮恢复工作流定时任务的保存入口，工作流输入默认值与必填校验遵循后端的参数 `name`；Telegram 手动接入复用现有 onboarding 接口并消费连接结果。Skills、聊天初始化/空 Dock、模型设置和远程访问均区分加载失败与真实空状态，提供重试，并保留已加载数据。独占读取按组件生命周期取消或隔离过期结果，不取消共享会话目录请求。新建计划改用统一 Dialog 管理初始焦点、Tab 循环、Escape 和焦点归还，提交期间防重入和关闭。手机从工作区跳转普通路由时同步到主屏，资产专用映射和手动三屏切换保持。

新入口 `npm run test:ui-ux` 已接入 CI，采用现有 esbuild/Playwright 模式，运行真实页面组件和隔离 HTTP 边界，覆盖错误/重试、表单草稿或设备快照保留、并发重试去重、卸载取消、工作流计划创建/保存、技能错误反馈、手动接入失败重试及弹窗键盘操作。先完成 `npm run build` 并安装 Playwright Chromium；同时运行现有 `test:chat-dock` 和 `test:mobile-shell`，防止页面错误提示破坏真实分屏几何或移动路由。Telegram 测试使用模拟响应，不是外部机器人端到端验收；浏览器移动视口也不等于原生设备验收。

仍需处理的审查项（本轮未改实现）：

- 通知模板：按事件/渠道保留未保存草稿，保存响应不得覆盖后续输入；连接状态须区分已配置、在线、禁用和不支持，发送测试依据真实能力。责任范围为 `NotificationSettings.tsx`；验收慢响应、切换往返及旧连接配置。
- 聊天草稿与托盘：资源调用/工作流参数应随会话草稿保留；不同分屏的工具托盘应互斥，并有明确的打开/关闭焦点。责任范围为 `use-focus-composer.ts`、`composer-drafts.ts` 和 `FocusSession.tsx`；验收切标签恢复、双分屏点击及全键盘操作。
- 资产：搜索/筛选/预览需拒收迟到响应。责任范围为 `AssetsPage.tsx`；验收旧请求后返回时仍显示最新选择。
- 移动服务器：连接、配对、本机切换需统一并发策略，旧响应不得清掉当前忙碌状态或再次导航。责任范围为 `MobileServerSettings.tsx`；用可控桥接 Promise 验证。
- 其他自建弹窗与外观单选组：逐个补齐 Dialog 焦点契约及 RadioGroup 方向键操作；不能由已迁移的新建计划弹窗替其他入口宣称通过。
- 可发现性建议：单标签下说明分屏前置条件或在侧栏增加方向打开；移动二级标签提供横向滚动提示；工具入口与术语需统一。这些是设计建议，未实施导航重排。

### 思考等级契约与项目侧栏（2026-10-11）

模型切换响应保留 `ok`，并恢复模型身份、思考能力、有效等级及上下文用量；思考 GET/PUT 同时提供 `availableLevels` 和 `availableThinkingLevels`，保留原 `status`/`supported` 字段，补齐前端状态字段。未加载会话仅归一读取结果，不重写日志；不支持的写入返回 `unsupported_thinking_level` 且不改变历史。前端将能力绑定到模型，按模型和选择修订拒收旧响应，兼容旧的仅 `ok` 响应，并在弹层打开时重新查询；按钮与滑块共用有效显示值，不再把旧能力和新模型混用。

项目目录沿用 `pisper-workspace-order` 的 `order` 作为保留列表，删除最后一个会话不会移除项目。折叠以可选 `collapsed` 字段保存在同一 v1 偏好中，旧值默认展开，没有新数据路径。显式删除项目成功后才移除目录、名称及折叠记录；一键收起/展开只控制项目组。旧 `order` 无法区分曾被旧 UI 显式删除的空目录，可能重新显示仍被记住的项目，可用显式删除入口移除。

新增 `npm run test:thinking` 和 `npm run test:sidebar-projects` 并接入 CI。前者包含真实控件/命令 Hook 的浏览器回归和隔离 Rust HTTP 的模型切换、非法写入、冷读及重启契约；后者覆盖最后会话删除、空项目新建、显式删除、搜索、排序、折叠与跨端口恢复。协议改动同时运行 Runtime 与 TUI 检查。本轮没有调用真实外部模型，用户开发页只读确认 K3 的五个档位已可选择，保持其原等级。

修复前的打包审查记录：审查范围为 v0.5.81、v0.6.0 与当时本地脚本，Rust 桌面及独立 CLI 本地入口支持 Windows x64/Mac arm64，但统一发布与资产清单只交付 Windows Desktop；Mac Intel、Linux、独立 CLI/npm 和移动渠道未完整接入。移动构建由 preflight 拒绝缺失的 Node embedded 入口，桌面 debug 仍引用旧 Node sidecar，Rust 资源闭包也未恢复旧版内置 rg/fd。当时仅比较源码与既有产物；后续修复与验证见下节。

### Rust 移动库与发布资源修复（2026-10-11）

二进制 main 的领域装配迁入 lib；CLI 仍有独立入口，移动端只消费显式启动/关闭接口。状态、目录、生命周期、能力限制与回滚见 [移动本机 Runtime](mobile-local-runtime.md)。桌面开发入口先构建 Rust 并暂存资源，不再调用不存在的 Node sidecar。资源暂存每次清除自己拥有的生成目录；签名打包前验证真实语音 worker 和 rg/fd。现有发布源码守卫改为多渠道和移动资源 v2 行为合同，保留失败前不覆盖旧资源、签名要求、严格资产数量和脏工作区拒绝保护。

Android 宿主在创建 Iroh/DNS/TLS 任务前初始化进程级 Application/JavaVM 上下文，并配套固定摘要的 rustls Java AAR 与 ProGuard 保留规则。系统容器目录的别名只在壳入口解析，领域内部仍拒绝链接。`scripts/tauri.mjs` 使 Gradle 回调使用相同的移动编译环境，C 编译基线保持 Android API 24。模拟器验收入口为 `npm run test:android:runtime -- emulator-5562`，只接受名为 `pisper-rust-release-validation` 的独立 AVD，使用虚拟 Provider 配置。

Linux 构建 Android 时，QuickJS 的 bindgen 需要宿主 `libclang`；Ubuntu 发布和商店流水线安装 `libclang-dev`。`mobile-rust-env.mjs` 在 Linux 保留显式 `LIBCLANG_PATH`，未设置时使用系统库发现，同时继续传入 NDK sysroot 和 Android API 24 目标参数。iOS 发布流水线先通过 Homebrew 安装 `minisign` 校验共享 Runtime 签名；两个 iOS 流水线均在暂存归档前创建 `release/`，不依赖开发目录已有产物。

### 无项目会话与模型目录容量（2026-10-12）

Web 普通“新任务”不再继承最近项目；项目与分屏内的明确目录仍保留。创建协议以 `cwd: null` 表示未绑定目录，省略字段保留旧客户端的 Runtime 默认目录行为。Runtime 在数据目录内建立独立的 `chat-workspaces`，`session-meta.json` 的增量 `workspaceUnset` 字段控制摘要中的空目录；旧记录默认 false，不迁移、不改写原目录。用户选择目录后通过原 cwd 接口重新绑定并清除该标记。领域内部仍使用真实目录，避免向文件工具传入空路径或隐式使用上一个项目。验证覆盖创建、显式目录、旧客户端、重启与后续绑定。

模型目录保留发现接口明确返回的容量，未知容量不再在发现阶段写入固定值；路由前缀参与同 Provider 定义匹配，聚合网关也可回退到无歧义的内置原厂容量，但保留实际请求型号、协议、地址和凭据。手动配置优先于发现值，再回退到发布冻结的官方目录与已有模型定义。任意层路由前缀（例如 `xxx/claude-opus-5-5` 或 `gateway/team/claude-opus-5-5`）只按完整末段 ID 精确匹配官方模型，不模糊猜测相似名字；未配置容量时使用 1M，实际请求 ID 保持不变。快速配置、自动添加和批量添加不再把旧的 128k/200k、关闭思考等默认值写成手动覆盖；已有明确填写的容量和 API 仍然保留，不自动迁移。会话保存了已移除的型号时，控件明确标记不可用并保留重新选择入口，不自动映射到另一个可能计费不同的型号；新会话只绑定当前可用的默认型号。`test:thinking` 覆盖控件恢复与真实 HTTP 契约，Rust Provider 测试覆盖 1M、手动覆盖、前缀与缺失容量。

默认型号保留但凭据已清除时，恢复过程不再调用需要认证的模型绑定，Runtime 必须仍能启动配置界面；发送时继续执行可用模型校验。iOS 26.5 arm64 模拟器实际覆盖无凭据启动、三视图导航、无目录新建、冷启动恢复以及本机模拟模型的认证与流式回复。Mac ARM 原生 App 完成 WKWebView、IPC、流式对话和终端入口验证；Intel Runtime/TUI 使用 x86_64 目标在 Rosetta 下运行测试，这不等同于 Intel 真机 App 或 iOS 真机验收。Windows/Linux 按本轮范围排除。

### 发布前 Pi Rust 与官方目录更新（2026-10-12）

Rust 布局的统一 `npm run release -- patch` 在校验干净工作区、release 分支、远端同步及没有进行中的发布工作流后，依次执行 `scripts/update-pi-rust.mjs` 和 `scripts/update-pi-model-catalog.mjs`。若上游内容改变，先运行发布脚本测试、Pi 本地补丁回归与全部受影响渠道的检查，再确认发布未被并发任务冻结，提交并推送一次 `build(deps)` 源码更新；渠道检测使用该新提交，后续构建固定同一个 SHA。各平台工作流不独立请求浮动目录，避免同一版本内数据不同。网络、校验、补丁合并或检查失败均停止派发。检查失败后的源码变更保留在本地供审查，不创建版本或标签。

- 模型目录来源为 [Pi 官方 typed catalog](https://pi.dev/api/models?types=chat,image,classifier)，按官方 `x-pi-model-catalog-revision` 的 SHA-256 校验完整响应；协议见 [Pi catalog protocol](https://github.com/earendil-works/pi/blob/main/scripts/model-catalog-protocol.ts)。`runtime-rs/assets/pi-model-catalog.json` 记录来源、revision、MIT 许可证和完整数据，编译进同一 Runtime 库。API、容量、思考映射与兼容参数一起覆盖旧目录；用户显式配置与代理连接优先，未实现的新 API 不自动开放。联网只发生在更新脚本中，App、CLI 和移动端启动保持离线能力。
- Rust 引擎来源为独立维护的 [pi-rs](https://github.com/xiaotubaobiu/pi-rust)，不将其描述成 Pi 官方 Rust 发行。`runtime-rs/pi-rs-source.json` 固定 crates.io 版本与归档摘要。更新只选择未撤回的稳定版本，拒绝降级；旧原版、当前 Pisper 修订、新原版按文件三方合并，保留新增回归测试和补丁。修改 Runtime 依赖后更新 Runtime/Tauri 两份锁文件；Cargo 失败会恢复原目录、清单、锁文件和来源记录，冲突不会覆盖现有代码。下一次更新以新的原版为基准，继续保留仍存在的本地差异。

可独立运行 `npm run models:update` 和 `npm run pi:rust:update` 生成可审查的本地变更；这两条命令不提交、不推送、不发布。`npm run test:release` 包含断网、错误摘要、非法目录、幂等更新、稳定版本选择、补丁冲突、回滚及派发顺序测试。Runtime 测试验证目录可被当前 SDK 解析，`scripts/smoke-rust-provider-upgrade.mjs` 的本机 Anthropic 模拟服务拒绝非 adaptive、缺少 effort 或携带 temperature 的 Opus 5.5 请求；不得将模拟服务通过描述成真实供应商账号已验证。

`ProviderConfigStore` 通过 SDK 的模型运行时工厂接口在会话型号解析前注入新目录，避免新型号被旧表回退并写入历史。初始化阶段保留旧型号身份，会话建立后才应用发现目录与禁用过滤；禁用型号恢复后仍阻止发送，不静默切换到另一个可能计费的型号。无认证、无自定义连接的供应商只在配置页投影目录，避免为每次会话初始化重组所有不可用模型。该适配不修改供应商源码，桌面和移动端共用同一生命周期入口。
