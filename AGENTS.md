# AGENTS.md

本文件是 Pisper 仓库的规则中枢。保留跨模块强制边界；具体环境、验证矩阵和专项设计以脚本、测试、CI 及实际代码为准。

## 先读与执行顺序

开始改动前必须：

1. 打开 `package.json`、相关 `Cargo.toml` 和目标目录，确认路径、命令与测试入口真实存在。
2. 涉及协议、存储、命令、权限或目录职责时，先定位契约来源及覆盖测试。
3. 声明本次验证范围及不运行的项目；完成后如实报告通过、失败、跳过和未执行项。
4. 检查目标目录是否有更具体的 `AGENTS.md`；文档与代码冲突时，以脚本、测试和 CI 为执行事实来源，并同步修正文档。

规则分级：

- **强制边界：** 安全、协议兼容、发布流程，以及“必须”“禁止”“不得”“只能”的要求。
- **默认建议：** “默认”“优先”“建议”“可以”“应”的组织方式；替代方案需说明职责、依赖方向与取舍。
- **迁移债务：** 新代码不得扩大偏差；优先治理本次涉及区域，不夹带全仓库重写。

## 项目与目录路由

Pisper 包含 React Web、Node.js Runtime、Tauri 2 桌面/移动壳和 Rust TUI。源码开发、验证和 SEA 构建以 Node.js 24 为基线；不要从 `engines` 推断完整 Runtime 支持范围。

| 路径 | 职责与边界 |
| --- | --- |
| `src/app/` | 启动、路由、Provider、导航与跨领域编排，不承载领域内部规则 |
| `src/features/<area>/` | 领域页面、模型、API、Store 与工具；跨 Feature 只通过明确公共接口协作 |
| `src/components/ui/` | 无业务语义的基础组件，不得依赖 Feature |
| `src/components/app/` | 应用级通用组合组件，通过参数、回调和插槽接收业务数据 |
| `src/lib/`、`src/hooks/` | 无业务语义的工具、HTTP 客户端、平台适配与 Hook，不反向依赖 Feature |
| `src/stores/`、`src/types/` | 无单一领域归属的客户端状态和类型 |
| `src/locales/{zh-CN,en-US}/` | 双语翻译资源 |
| `runtime/http/` | 请求校验、权限、服务调用、SSE 编排与响应，不拥有领域规则 |
| `runtime/services/` | 领域规则、状态机、持久化、Repository 与迁移 |
| `runtime/runtime/` | Pi 引擎与领域服务的窄接口装配 |
| `runtime/tools/app/` | 应用级代理工具；经既有入口注册，不耦合整个 Runtime |
| `runtime/tests/` | Runtime、协议、前端纯逻辑、构建与源码守卫测试 |
| `shared/` | 确需跨客户端或 Runtime 复用的环境中立协议、数据与纯逻辑 |
| `src-tauri/` | Tauri 命令注册、桌面桥接、更新器与平台接线 |
| `src-tui/` | Rust TUI 入口、渲染与终端交互 |
| `crates/` | 确有独立边界的 Rust 库与平台适配 |
| `scripts/` | 构建、发布、开发工具与迁移入口，不承载产品领域规则 |
| `packages/pisper/` | npm 安装器、启动器与发布源码清单 |
| `docs/` | GitHub Pages 官网、隐私与支持页，以及线上读取的 `latest-app.json`、`sponsors.json`；不存放开发或设计文档 |
| `dist/`、`release/` | 构建和打包产物，不手工维护 |

路径别名：`@/*` → `src/*`，`@shared/*` → `shared/*`。业务所有权优先于复用次数；多个调用方不自动意味着应提升为全局模块。

## 核心开发边界

### 结构与依赖

- 模块按业务边界、共同变化原因、依赖方向和可测试性组织，不按固定行数机械拆分。
- 基础组件不依赖 Feature；跨 Feature 不访问内部实现；不得新增循环依赖。
- 公共入口不无选择导出内部实现，不引入重型依赖或初始化副作用；纯类型使用 `export type`。
- 拆分或合并须明确状态所有者、生命周期和窄接口，不以移动文件或搬移共享 `this` 冒充解耦。
- 第三方同步目录与业务代码隔离；生成文件通过生成源和脚本维护。
- 新文件遵循现有命名：组件 `PascalCase.tsx`、Hook `useXxx.ts`、Store `xxx-store.ts`、前端模块 `kebab-case.ts`、脚本/共享模块 `kebab-case.mjs`、Rust 模块 `snake_case.rs`。
- 代码注释默认使用中文并解释“为什么”；上游、生成文件和公共 API 可沿用既有语言。

### 前端

- 使用严格 TypeScript、`@/` 别名、Tailwind 与 shadcn/ui；`src/**` 禁止相对父级导入。
- 不向 `src/index.css` 添加页面、功能或控件级语义类；优先组合基础组件和工具类。
- 用户可见文案使用可静态检查的 i18n 键，`zh-CN` 与 `en-US` 必须同步。
- 保留 `vite.config.ts` 的 `build.target: 'safari16'` 和构建链中的 `scripts/check-dist-compat.mjs`，不得改成 `safari15`。
- 持久化页面状态使用 `src/lib/page-state-storage.ts`，新键登记到 `shared/browser-preferences.mjs`；不得新增仅写 `localStorage` 的偏好。
- 页面和展示组件不得直接拼接业务 API 或调用 `fetch`；由领域 API 模块复用统一 HTTP 客户端。
- 新交互须支持键盘、焦点管理、语义标签及加载、空数据、错误和禁用状态。

### Runtime、协议与共享层

- Runtime 使用 ESM `.mjs`；API 处理器放在 `runtime/http/`，领域逻辑放在 `runtime/services/`，Pi 接线放在 `runtime/runtime/`。
- 外部输入先按不可信 `unknown` 校验、归一化和限制长度，再进入业务层；不得用断言掩盖非法数据。
- HTTP/SSE、存储和原生桥接字段、状态、错误码及终止事件保持稳定；变更优先向后兼容，并补契约或迁移测试。
- Runtime 输出必须是严格有效的 JSON/UTF-8，同时兼容 `JSON.parse` 和 `serde_json`。
- 公共协议改动须评估 Web、TUI 和受影响原生客户端，不得只验证单一客户端。
- 新工具在 `runtime/tools/app/` 按既有 `defineTool()` 模式实现并经入口注册，不绕过审批或耦合整个 Runtime。
- 未完成能力通过 capability 和结构化 `unsupported` 暴露，不以空数据或简化结果伪装成功。
- `shared/` 不依赖 DOM、React、Node 专属 API 或原生接口；公共接口变化时同步 `.d.mts` 和 `tsconfig.jscheck.json` 覆盖。
- 配置在单一入口解析与校验；持久化沿用原子写入、权限和迁移机制。

### 状态、异步与资源

- 同一远程实体只能有一个权威数据所有者；草稿和派生数据须明确提交、回滚及失效规则。
- 请求、SSE、Worker、定时器、音频和事件监听须明确所有者、取消与清理路径；模块导入不得隐式启动长期副作用。
- 异步任务不得遗留未处理 Promise、异常或后台任务；处理重复启动、超时、重试、乱序响应和取消。
- 新增或升级依赖须同步锁文件并在变更说明中记录原因；资源、模型和二进制须记录来源、版本、许可证、摘要与打包路径。
- 用户可见行为、命令、配置、协议、存储或目录边界变化时，同步测试、帮助文本、文档和必要发布说明。

### 安全与平台红线

- 不得提交密钥、`release/` 产物、`src-tauri/binaries/`、`.pisper/` 用户代理状态或真实个人数据。
- 外部命令使用参数数组和允许的可执行文件，禁止拼接未校验的 Shell 字符串；文件操作限制在授权目录并防路径穿越。
- HTML、Markdown、图片和远程 URL 沿用清洗、白名单与安全引用机制，不为显示内容放宽 CSP。
- 日志、错误、遥测和测试输出不得包含凭据、Cookie、私钥、完整个人路径或用户会话内容。
- 桌面端只能使用 Tauri + Node SEA；不得引入 Electron。
- 跨端共享规则须评估桌面、Android、iOS；平台能力集中判断，不支持时明确返回 `unsupported`。
- 语音模型等共享 catalog 不得擅自增加平台专属副本或改变语义。
- 缺少平台、设备或签名环境时明确报告未验证项，不得把浏览器模拟或他人待执行验收写成已通过。

## 常用命令

完整命令以 `package.json` 的 `scripts` 为准；只引用实际存在的入口。

```bash
npm ci                       # 可复现安装；仓库只使用 npm 和 package-lock.json
npm run dev                  # Vite 中间件，默认 127.0.0.1:5173，提供 Web 与 API
npm run build                # 前端构建、体积预算与 Safari 兼容检查
npm run preview              # 通过生产服务器提供已构建资源
npm run check                # 类型、lint、i18n、格式和启动闸门
npm test                     # runtime/tests/*.test.mjs 全部测试
npm run test:ui              # UI 冒烟；其他 test:*:ui 按影响范围选择
npm run sidecar:sea          # 构建 Node SEA 并准备 Runtime 闭包
npm run sidecar:sea:smoke    # 验证暂存的 SEA、Runtime 和 API
npm run tui:check            # TUI cargo check
npm run tui:test             # TUI cargo test
npm run desktop:webview:build
npm run npm:pack:check
npm run build:android
npm run build:ios
npm run release -- patch     # 发布也可传明确版本号
```

开发 Web 由 `runtime/index.mjs` 嵌入 Vite 中间件；桌面端为 Tauri 壳 → `pisper-sidecar`（Node SEA）→ `sidecar-runtime/`。

## 测试与验证

按改动范围选择检查：

- TypeScript/界面：受影响范围的类型、lint、i18n；跨区域或构建配置运行 `npm run check`。
- Runtime/工具/路由：相关 `runtime/tests/` 测试；公共线协议还须验证 Web、TUI 与契约。
- TUI、Tauri、SEA、npm、Android、iOS：执行对应格式、检查、测试、构建及设备验收。
- 纯文档：检查差异、本地链接、命令引用、规则一致性和 `git diff --check`，不机械运行全量构建。
- 新测试必须接入实际执行入口与 CI；优先扩展已有测试，不引入第二套测试框架。
- 测试不依赖真实凭据、个人目录或外部网络；先停止任务再清理环境，不用任意等待掩盖竞态。
- 失败时区分回归、环境、时序和过时断言；不得跳过检查、删除行为保护或虚报通过。
- Rust 按改动范围运行 `cargo fmt`、`cargo check`、`cargo test` 和 `clippy`；避免无理由的 `unwrap()`、`expect()` 和 `unsafe`。

## 发布与协作

- 发布只包含实质性行为、产物、安全或兼容性变更；纯文档、格式化和版本记账不单独创建版本。
- 发布以 `scripts/release.mjs`、`scripts/release-policy.mjs` 和 GitHub Actions 为准；不得绕过脚本手改版本、建标签或重复发布。
- 发布前检查分支、工作区、标签和渠道实际变更；工作流运行期间冻结发布分支，失败先查日志与远端状态。
- 分支集成禁止 `git merge` 和 merge commit，只用 `git rebase` 或 `git cherry-pick`；共享/发布分支禁止强制推送和改写他人提交。
- 变更说明包含目的、范围、验证、风险、迁移与回滚；不夹带无关格式化、重命名或目录搬迁。
- 使用子代理时：子代理运行限定测试，集成者运行完整检查；重构子代理不得修改源码守卫。
