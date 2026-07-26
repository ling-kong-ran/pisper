<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/vesper-logo.svg" width="112" alt="Vesper 项目标志" />
</p>

<h1 align="center">Vesper</h1>

<p align="center"><strong>暮色落下，灵感仍醒着。</strong></p>
<p align="center">本地优先的多 Agent 工作台，让会话、工具、记忆与工作流在同一片星图中并行运转。</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Vite-17141F?style=flat-square&logo=vite&logoColor=F59E0B" alt="Vite" />
  <img src="https://img.shields.io/badge/Tailwind_CSS-17141F?style=flat-square&logo=tailwindcss&logoColor=F59E0B" alt="Tailwind CSS" />
  <img src="https://img.shields.io/badge/Electron-17141F?style=flat-square&logo=electron&logoColor=F59E0B" alt="Electron" />
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/vesper/releases/latest">
    <img src="https://img.shields.io/badge/下载桌面版-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="下载 Vesper 桌面版安装包" />
  </a>
</p>
<p align="center"><sub>可直接下载安装包，无需克隆源码或配置 Node.js 开发环境。</sub></p>

<p align="center">
  <a href="#about">暮星之名</a> ·
  <a href="#glance">一眼 Vesper</a> ·
  <a href="#lightweight-prompt">轻量 Prompt</a> ·
  <a href="#capabilities">能力星图</a> ·
  <a href="#desktop-pet">桌面宠物</a> ·
  <a href="#architecture">代码星图</a> ·
  <a href="#stack">技术栈</a> ·
  <a href="#start">从这里启程</a> ·
  <a href="#contributing">参与贡献</a>
</p>

---

<a id="about"></a>

## ✦ 暮星之名

**Vesper**，取意于黄昏与晚星。

白昼退场之后，仍有一束光守望尚未完成的工作。Vesper 希望成为这样的存在：安静、清醒、始终在场，把分散的模型、工具与上下文收拢成一张可见、可控、可继续生长的工作星图。

Vesper 是一款**本地优先的多 Agent 工作台**。你可以让多个独立会话同时推进，让每个 Agent 拥有自己的模型、执行模式、上下文与工作目录；通过可拖拽标签与 `Split Left` / `Split Right` 左右分屏，将任务编排成可随时调整并自动恢复的 IDE 式工作区。

> 让每个 Agent 各行其轨，让每一次思考都有归处。

- **并行而不混乱**：会话彼此独立，状态、权限与上下文清晰可见。
- **轻量而不减配**：动态 Prompt 只携带当前任务真正需要的工具，让普通编码会话保持极致轻量。
- **自动化而不越界**：定时任务与工作流负责重复劳动，敏感操作仍由你决定。
- **记住而不喧哗**：偏好、事实、决策与文件关系沉淀为本地「星忆」。
- **连接而不失守**：MCP、插件与外部渠道可以扩展能力，也始终受权限边界约束。

---

<a id="glance"></a>

## ✦ 一眼 Vesper

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Vesper 暗色主题动态欢迎页" />
</p>

![Vesper 产品演示](./docs/show.gif)

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Vesper 多会话 Dock 分屏工作区" />
      <br />
      <sub><strong>Dock 分屏工作区</strong> · 用标签、左右拆分与拖拽停靠并行推进任务</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Vesper 星忆视图" />
      <br />
      <sub><strong>星忆</strong> · 让值得保留的思考与决定持续发光</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Vesper 工作流编辑器" />
      <br />
      <sub><strong>工作流</strong> · 把一次设想编排成可运行、可复用的路径</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Vesper 渠道视图" />
      <br />
      <sub><strong>双向渠道</strong> · 让 Vesper 抵达飞书与个人微信</sub>
    </td>
  </tr>
</table>

---

<a id="lightweight-prompt"></a>

## ✦ 动态 Prompt · 极致轻量

Vesper 不会在每次模型调用时，把所有工具、MCP、视觉、浏览器、星忆与多 Agent 规则一股脑塞进上下文。运行时会保留一个稳定、精简的基础 Prompt，并根据用户当前的明确请求，按需追加对应工具 Schema。

<table>
  <tr>
    <td width="33%" valign="top">
      <strong>轻量默认上下文</strong><br />
      普通编码会话只加载读写、搜索、Shell 与任务进度等高频工具。当前基准下，固定输入约为 <strong>2,979 tokens</strong>，相比全量注入约减少 <strong>58.7%</strong>。
    </td>
    <td width="33%" valign="top">
      <strong>能力按需唤醒</strong><br />
      Web Search、浏览器、视觉生成、星忆、MCP 与 Subagent 等能力，仅在用户明确需要时追加；新的普通请求会自动回到轻量基线。
    </td>
    <td width="33%" valign="top">
      <strong>Prompt Cache 友好</strong><br />
      System Prompt 与高频工具 Schema 保持稳定前缀，低频工具只追加在末尾，尽可能复用 Provider 的 Prompt Cache，减少重复输入与延迟。
    </td>
  </tr>
</table>

- **显式 Skills**：可将大型专业流程设为仅通过 `/skill:name` 调用，默认不向模型暴露名称、描述或正文，实现零常驻上下文开销。
- **可信意图路由**：只依据最新用户请求和明确附件元数据激活能力；网页、文件、工具输出、星忆或 Agent 结果中的文本不能偷偷扩张工具集。
- **否定语义保护**：诸如“不要使用浏览器”“Do not use MCP”的请求不会误激活对应工具。
- **权限仍是硬边界**：动态激活只决定本轮需要哪些工具，不会绕过工具开关、执行模式、工作区、沙箱或审批策略。

> Token 数值使用 Vesper 的保守估算 `ceil(characters / 4)`，来自当前默认完整模式的可复现基准；实际 Provider 计费、缓存命中与延迟收益会因模型和服务商而异。

---

<a id="capabilities"></a>

## ✦ 能力星图

| 模块 | 能力 |
| :--- | :--- |
| **多会话对话** | 多个独立 Agent 会话并行运行；模型、执行模式、上下文与工作目录彼此分明，支持标签分组、左右拆分、拖拽停靠、尺寸调整与布局恢复。 |
| **Agent Runtime** | 以 Pi Coding Agent 为运行内核，支持结构化工具活动、Goal、可复用 Skills 与隔离的 Subagent 委派。 |
| **动态 Prompt** | 保持稳定、轻量、缓存友好的基础 Prompt，仅按用户明确意图追加低频工具 Schema；支持零常驻开销的显式 Skills。 |
| **工具与 MCP** | 将内置工具、应用插件与 MCP 服务接入同一能力层，并通过不暴露凭据的结构化配置守住连接边界。 |
| **星忆** | 以轻量本地 SQLite 保存偏好、事实、决策与任务；按工作空间隔离，支持搜索、主动点亮、编辑与对话提取。 |
| **多模态** | 阅读图片、文档与代码，也可通过已配置的 OpenAI 兼容、Gemini、Imagen、Veo 或 xAI 模型生成及编辑视觉内容。 |
| **自动化** | 用定时任务与可视化工作流承接重复劳动，支持模型选择、重试、超时、失败策略、执行历史与完成通知。 |
| **双向渠道** | 连接飞书与个人微信，为每个渠道配置回复模型、工作目录、附件传输与可复用通知模板。 |
| **桌面宠物** | 在 Vesper 内搜索、安装和切换 Petdex 兼容宠物，让等待、思考、工具执行、完成与失败等 Agent 状态映射为实时动画。 |
| **桌面应用** | 提供 Electron 原生窗口、单实例运行、品牌图标、应用内更新日志，以及基于 GitHub Releases 的更新能力。 |
| **安全边界** | 会话级 `只读 / 工作区 / 完全访问` 执行模式、工作区外单次授权、服务端凭据脱敏，以及独立于仓库之外的本地用户数据存储。 |

> **本地沙箱说明**：默认“工作区”模式使用开源的 [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) 限制 Shell 的文件写入、凭据与网络访问；Windows 首次启用需要一次 UAC 来安装低权限账户和网络隔离设施，Agent 本身不会获得管理员权限。Windows 上同一工作区的多个会话可以并发执行，不同工作区会安全地排队切换单一授权，避免共享沙箱账户跨项目写入或在大型目录上反复传播 deny ACL。沙箱初始化失败时 Vesper 会阻止执行，而不会静默回退到完整本机权限。该运行时目前仍是 Beta Research Preview，沙箱能降低风险，但不能保证未知代码绝对安全。

---

<a id="desktop-pet"></a>

## ✦ 桌面宠物 · 陪伴 Agent 工作

Vesper 原生支持 [Petdex](https://petdex.dev) 兼容桌面宠物。你可以直接在 **设置 → 桌面宠物** 中搜索、安装、切换和启停宠物，不需要安装 Petdex CLI、Desktop、hooks 或其他伴随进程。

- **跟随 Agent 状态**：等待回复、思考审阅、运行工具、完成任务与执行失败都会映射为对应动画；点击宠物还会触发跳跃互动。
- **原生桌面悬浮**：Electron 使用独立透明窗口，支持拖动、置顶、多显示器位置记忆和 `20%–100%` 透明度；主窗口隐藏到托盘后，宠物仍会继续显示。
- **Web 页面陪伴**：浏览器版本使用页面内可拖动悬浮层，沿用相同的宠物资源与 Agent 状态动画，并在浏览器本地保存位置。
- **安全安装资源**：Vesper 只通过 HTTPS 白名单下载资源，并在保存前验证文件大小、PNG/WebP 结构、精灵帧网格和本地路径边界；社区宠物资源不会预打包进 Vesper。
- **贴近角落而不打扰**：首次启用时默认显示在主显示器或页面右下方，拖动后的自定义位置会持续保留。

详细协议、资源格式与安全边界请参阅 [`docs/petdex-integration.md`](./docs/petdex-integration.md)。

---

<a id="architecture"></a>

## ✦ 代码星图

```text
vesper/
├─ .github/              # CI、Release Notes 与全平台发布工作流
├─ docs/                 # 项目文档、截图与品牌资源
├─ electron/             # Electron 主进程与安全 preload
├─ public/               # 公共静态资源
├─ scripts/              # 图标生成、桌面打包、i18n 检查与版本发布
├─ shared/               # 前后端共享的工作流图逻辑
├─ server/
│  ├─ http/              # HTTP API、SSE 与静态资源响应
│  ├─ prompts/           # Agent 系统提示词与运行时身份注入
│  ├─ runtime/           # Pi Agent 会话与模型运行时
│  ├─ security/          # 凭据与输出脱敏
│  ├─ services/          # 渠道、星忆、工作流等领域服务与外部集成
│  ├─ storage/           # 本地持久化工具
│  ├─ tests/             # Node.js 测试
│  └─ tools/             # 内置工具与应用工具注册表
└─ src/
   ├─ app/               # 路由、Providers、导航、品牌与 i18n
   ├─ components/        # UI 组件、shadcn/ui、AI Elements
   ├─ features/          # 按页面拆分的功能模块
   ├─ hooks/             # 通用 React Hooks
   ├─ lib/               # Axios 客户端与格式化工具
   ├─ locales/           # 命名空间化的 i18n 资源
   ├─ stores/            # Zustand 客户端状态
   └─ types/             # 前端类型定义
```

---

<a id="stack"></a>

## ✦ 技术栈

前端已完成 TypeScript 化重构，采用模块化路由、状态与 UI 体系，便于扩展与协作。

| 层级 | 选型 |
| :--- | :--- |
| **语言与构建** | TypeScript、Vite、React 19 |
| **路由与状态** | React Router Data Mode、Zustand、TanStack Query、Axios |
| **UI 体系** | Tailwind CSS、shadcn/ui、Radix UI、Lucide、Sonner |
| **Agent / 富内容** | AI Elements、Streamdown、Shiki、React Markdown |
| **画布与布局** | React Flow（`@xyflow/react`）、Dockview、Resizable Panels |
| **国际化** | i18next / react-i18next，按命名空间拆分的 JSON 资源 |
| **运行内核** | Pi Coding Agent、Node.js、Electron |
| **质量门槛** | Oxlint、Prettier、TypeScript 类型检查、i18n AST 校验 |

本地质量检查：

```bash
npm run check   # typecheck + lint + i18n:check + format:check
npm test
npm run build
```

---

<a id="start"></a>

## ✦ 从这里启程

### 直接下载桌面版

无需从源码构建。前往 [GitHub Releases 最新版本](https://github.com/ling-kong-ran/vesper/releases/latest)，即可直接下载 Windows、macOS 或 Linux 安装包。

> [下载最新版 Vesper 桌面安装包 →](https://github.com/ling-kong-ran/vesper/releases/latest)

### 从源码运行

以下环境要求仅适用于开发、Web 版运行或自行打包桌面应用。

#### 环境要求

- Node.js 20 或更高版本
- npm
- 至少一个受支持的模型 Provider 和 API Key

#### 安装依赖

```bash
git clone https://github.com/ling-kong-ran/vesper.git
cd vesper
npm install
```

### Web 版

启动开发服务：

```bash
npm run dev
```

服务就绪后，终端会明确打印访问地址。请在浏览器中打开 `http://127.0.0.1:5173`。Vesper 默认不会自动创建浏览器页签；如有需要，可设置环境变量 `VESPER_OPEN_BROWSER=1`。

Web 版启动时会比较当前 Git commit 与 GitHub `main` 分支，而不是等待 Release Tag。若远端已有尚未同步的提交，左侧导航栏底部会显示落后的提交数；点击即可查看差异。该检查只负责提醒，不会强制刷新页面、下载文件或覆盖本地源码。

更新源码前，请先提交或暂存本地修改，然后执行：

```bash
git pull
npm install
```

构建并运行 Web 生产版本：

```bash
npm run build
npm start
```

### 桌面版

启动或打包桌面应用：

```bash
npm run desktop:dev
npm run desktop:pack
```

桌面版会在启动后自动检查更新，并在左侧导航栏显示新版本提醒。更新不会自动下载；只有在用户确认后才会开始下载，并可先查看应用内更新日志。

发布新版本时，脚本会更新并提交 `package.json` 与 `package-lock.json`，创建 Git Tag，再由 GitHub Actions 生成更新日志并发布 Windows、macOS 与 Linux 安装包：

```bash
npm run release -- patch
# 也可使用 minor、major 或明确版本号，例如 1.2.0
```

### 本地数据

Vesper 默认将配置、会话、星忆与运行数据保存在：

```text
~/.vesper/agent
```

可通过环境变量 `VESPER_AGENT_DIR` 指定其他目录。

### 验证

```bash
npm run check
npm test
npm run build
```

项目使用 Node.js 内置测试运行器（经 `tsx` 执行），测试文件位于 `server/tests/`。

---

<a id="contributing"></a>

## ✦ 与 Vesper 同行

欢迎提交 Issue 与 Pull Request：

- [报告问题](https://github.com/ling-kong-ran/vesper/issues)
- [提交 Pull Request](https://github.com/ling-kong-ran/vesper/pulls)
- [查看贡献者](https://github.com/ling-kong-ran/vesper/graphs/contributors)

提交修改前，请运行上方的 lint、测试与构建命令。请勿提交 API Key、机器人凭据、本地会话数据，或 `~/.vesper/agent` 中的任何文件。

---

<a id="acknowledgements"></a>

## ✦ 致谢 · 星光所自

Vesper 以 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 为运行内核，并由 Node.js、React、TypeScript、Vite、Tailwind CSS、shadcn/ui、React Router、Zustand、TanStack Query、React Flow、i18next 等开源项目共同照亮。

感谢贡献者：

- [@mik-myp](https://github.com/mik-myp) — 前端 TypeScript 架构、shadcn/ui / AI Elements、Zustand 与 i18n 重构（[#1](https://github.com/ling-kong-ran/vesper/pull/1)）

<p align="right"><a href="#top">返回顶部 ↑</a></p>
