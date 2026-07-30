<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper 项目标志" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>Pi 驱动，Agent 低语。</strong></p>
<p align="center">让复杂在后台静默运转，让每一个任务得到清晰回应。</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Tauri-17141F?style=flat-square&logo=tauri&logoColor=F59E0B" alt="Tauri" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest">
    <img src="https://img.shields.io/badge/下载桌面版-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="下载 Pisper 桌面版" />
  </a>
</p>

<p align="center">
  <a href="#overview">简介</a> ·
  <a href="#preview">界面</a> ·
  <a href="#features">功能</a> ·
  <a href="#desktop-pet">桌面宠物</a> ·
  <a href="#install">安装</a> ·
  <a href="#tui">终端客户端</a> ·
  <a href="#development">开发</a> ·
  <a href="#sponsors">赞助</a> ·
  <a href="#license">许可</a>
</p>

---

<a id="overview"></a>

## 简介

一个任务开一个窗口、上下文来回复制、工具配置散落各处——多 Agent 协作不该这么累。

**Pisper 把多个 AI Agent 汇入一个桌面工作台。** 模型、上下文、工具与权限各自归位，复杂协作在后台安静发生；你只需专注眼前的任务。每个会话独立运行，支持标签分组、分屏与拖拽停靠，关掉重开，工作现场依旧。

**产品亮点：**

- **极简无声，效率自明** — 会话、工具、记忆与执行状态归于一处；少切窗口、少配入口、少喂无关上下文。
- **按需应答，能力不喧宾夺主** — 渐进式披露保持高频工具轻量，在任务需要时热加载低频工具 Schema，并按场景展开完整 Skill。
- **多声部并行，主线不停** — Subagent 在隔离上下文中非阻塞运行；主 Agent 仍可继续回复、接收新指令或分派其他任务。
- **任务完成，自有回响** — Subagent 持久保存结果并自动唤醒父 Agent 接续推理，无需轮询，不让成果停在一条被动通知里。
- **各守其界，协作有序** — 每个会话独享模型、上下文、目录与权限；动态加载始终服从执行模式、沙箱和审批。

### 三步上手

1. [下载桌面版](https://github.com/ling-kong-ran/pisper/releases/latest)（Windows / macOS / Linux，免装 Node.js）
2. 配置任意一个模型 Provider 与 API Key
3. 新建会话，开始并行工作

<a id="preview"></a>

## 界面

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Pisper 暗色主题欢迎页" />
  <br /><sub><strong>暗色工作台</strong> · 每个会话拥有独立模型、上下文、目录与权限</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat.png" alt="Pisper 单会话工作区" />
      <br /><sub><strong>会话工作区</strong> · 聚焦任务、工具状态与上下文用量</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Pisper 多会话 Dock 分屏工作区" />
      <br /><sub><strong>Dock 分屏</strong> · 标签、横纵分屏与拖拽停靠</sub>
    </td>
  </tr>
</table>

### 工作台

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/history.png" alt="Pisper 历史会话页" />
      <br /><sub><strong>历史会话</strong> · 搜索、重命名与恢复任意布局</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/assets.png" alt="Pisper 资产库" />
      <br /><sub><strong>资产</strong> · 汇总图片、文件、链接与 Agent 产物</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Pisper 渠道页" />
      <br /><sub><strong>双向渠道</strong> · 连接飞书与个人微信</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/schedules.png" alt="Pisper 定时任务页" />
      <br /><sub><strong>定时任务</strong> · 配置频率、执行模式与通知</sub>
    </td>
  </tr>
</table>

### 能力

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/plugins.png" alt="Pisper 工具策略页" />
      <br /><sub><strong>工具</strong> · 按风险与执行模式管理权限</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Pisper 星忆视图" />
      <br /><sub><strong>星忆</strong> · 在持久记忆星图中检索事实与决策</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/mcp.png" alt="Pisper MCP 服务页" />
      <br /><sub><strong>MCP</strong> · 管理服务、工具权限与调用记录</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/skills.png" alt="Pisper 技能页" />
      <br /><sub><strong>技能</strong> · 安装并控制可复用能力包</sub>
    </td>
  </tr>
</table>

### 自动化

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflows.png" alt="Pisper 工作流列表" />
      <br /><sub><strong>工作流</strong> · 从预设或草稿组织自动化</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Pisper 工作流编辑器" />
      <br /><sub><strong>可视化编排</strong> · 连接 Prompt、文件、MCP 与通知节点</sub>
    </td>
  </tr>
</table>

### 设置

<p align="center">
  <img src="./docs/shots/config.png" alt="Pisper Provider 与模型设置" />
  <br /><sub><strong>Provider 与模型</strong> · 独立管理协议、地址、凭据与模型目录</sub>
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/config-notifications.png" alt="Pisper 通知设置" />
      <br /><sub><strong>通知</strong> · 为会话、任务和工作流定制模板</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/config-interface.png" alt="Pisper 界面设置" />
      <br /><sub><strong>界面</strong> · 语言、密度与显示偏好</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/config-desktop-pet.png" alt="Pisper 桌面宠物设置" />
      <br /><sub><strong>桌面宠物</strong> · 从 Petdex 搜索、安装与管理宠物</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/config-updates.png" alt="Pisper 应用更新页" />
      <br /><sub><strong>应用更新</strong> · 查看版本状态与更新渠道</sub>
    </td>
  </tr>
</table>

<a id="features"></a>

## 功能

从对话到自动化，Pisper 覆盖 Agent 工作的完整链路：

| 模块 | 能力 |
| :--- | :--- |
| **多会话对话** | 独立 Agent 会话、模型与权限；支持标签分组、分屏、拖拽停靠与布局恢复。 |
| **Agent Runtime** | 基于 Pi Coding Agent，支持工具活动、Goal、Skills 与异步隔离 Subagent。 |
| **渐进式披露** | 高频工具保持稳定，低频工具 Schema 可在当前会话按意图热加载，无需重启会话。 |
| **Skills** | 兼容 Agent Skills 标准；上下文默认只保留名称与描述，完整 `SKILL.md` 在匹配任务或显式调用时加载。 |
| **异步 Subagent** | 后台非阻塞执行独立任务，结果持久保存，完成后自动唤醒父 Agent 接续工作。 |
| **工具与 MCP** | 统一管理内置工具、插件和 MCP 服务，不向界面层暴露凭据。 |
| **星忆** | 用 SQLite 保存并检索偏好、事实、决策与任务，越用越懂你。 |
| **多模态** | 阅读图片、文档与代码，并通过已配置模型生成或编辑视觉内容。 |
| **自动化** | 定时任务与可视化工作流，支持重试、失败策略、历史与通知。 |
| **双向渠道** | 连接飞书与个人微信，离开电脑也能随时召唤 Agent。 |
| **Web 预览** | 在 Dock 面板中预览外部网页，必要时回退到系统浏览器。 |
| **桌面应用** | Tauri 2 系统 WebView 单实例应用，支持托盘、签名更新与 GitHub Releases。 |
| **安全边界** | 会话级 `只读 / 工作区 / 完全访问`、单次审批、凭据隔离与本地数据边界。 |

### 能力轻声应答：渐进式披露

Pisper 不让所有能力同时涌入上下文，而是让工具与知识沿着任务逐层展开：

- **热路径常驻** — 文件、搜索、编辑与终端等高频工具保持稳定，维持可缓存的轻量基础 Prompt。
- **工具热加载** — Web Search、浏览器、星忆、MCP 与 Subagent 等可选能力可按意图发现，并把最相关的 Schema 即时加入当前会话。
- **Skill 按需展开** — 默认只暴露 Skill 的名称与描述；任务匹配或显式调用时才读取完整说明、脚本与参考资料。

这套机制既减少长会话中的固定 Token，也避免无关工具干扰模型决策。动态加载始终受工具开关、执行模式、沙箱和审批策略约束。

> 当前基准下，固定 Prompt 约为 **2,979 tokens**，相较全量注入减少约 **58.7%**。实际计费与缓存收益因模型和服务商而异。

### 多声部并行：非阻塞 Subagent

一个边界清晰的任务，可以交给一个隔离 Subagent，而主 Agent 无需停下脚步。Subagent 在后台继承当前模型、推理级别、安全工具和工作区边界；父会话不必轮询，也不会因后台任务仍在运行而阻塞回复。

完成结果不会沉入无人查看的通知：父 Agent 正在运行时，结果直接汇入当前推理；父 Agent 空闲时，Pisper 会通过隐藏的内部消息自动唤醒下一轮，让每一次后台协作都回到任务主线。

> **沙箱说明：** 默认“工作区”模式使用 [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) 限制 Shell 写入、凭据与网络访问。Windows 首次启用需要一次 UAC；初始化失败时 Pisper 会阻止执行，不会静默回退到完整权限。该运行时仍处于 Beta Research Preview。

<a id="desktop-pet"></a>

## 桌面宠物

工作台不必冷冰冰。Pisper 原生支持 [Petdex](https://petdex.dev) 兼容宠物——Agent 在思考，宠物在踱步；任务完成，它会告诉你。在 **设置 → 桌面宠物** 中即可搜索、安装、切换与启停，无需 Petdex CLI 或额外进程。

- Agent 的等待、思考、工具执行、完成与失败状态会映射为宠物动画。
- Tauri 桌面版使用独立透明窗口，支持拖动、置顶、多显示器位置记忆和 `20%–100%` 透明度；隐藏主窗口后仍可显示。
- Web 版使用页面内可拖动悬浮层，关闭页面后随之退出。
- 资源仅从 HTTPS 白名单下载，并校验大小、格式、精灵网格与路径边界；社区宠物不会预打包。

详见 [`docs/petdex-integration.md`](./docs/petdex-integration.md)。

<a id="install"></a>

## 安装

### 桌面版（推荐）

前往 [GitHub Releases](https://github.com/ling-kong-ran/pisper/releases/latest) 下载 Windows、macOS 或 Linux 安装包，开箱即用，无需额外安装 Node.js。

#### macOS 无法打开

Pisper 当前尚未经过 Apple 公证。请确认应用来自官方 Releases，并优先在 **系统设置 → 隐私与安全性** 中选择 **仍要打开**。若没有该选项，可执行：

```bash
sudo xattr -rd com.apple.quarantine /Applications/Pisper.app
```

请仅对从官方 Releases 下载并放入 `/Applications` 的应用使用此命令。

#### Linux AppImage 无法启动

```bash
chmod +x Pisper_*_linux_x86_64.AppImage
./Pisper_*_linux_x86_64.AppImage
```

缺少 FUSE 时安装 `libfuse2` 或 `libfuse2t64`，也可改用 `.deb`：

```bash
sudo apt install ./Pisper-*-linux-amd64.deb
```

<a id="tui"></a>

### 终端客户端（TUI）

Pisper 同时提供 Rust + Ratatui 终端客户端。每次启动 TUI 时，主区域都会先显示 Pisper 终端品牌标识；开始交互后，品牌画面自动让位给消息流。模型提供 reasoning token 时，TUI 会在回答前实时展开 Thinking，并用事件驱动的终端 spinner 标识活跃状态。TUI 复用桌面版的 Node SEA sidecar、Agent runtime、会话、Tools、Skills、MCP、沙箱和审批链。

安装桌面版后，可在 **设置 → 界面设置 → Pisper 终端命令** 中安装或卸载 `pisper` 命令。Pisper 会使用当前用户目录并管理对应 PATH 项，不需要管理员权限；操作后需重启终端宿主。Windows 安装的是 `pisper.exe`，macOS 和 Linux 安装的是无扩展名的 `pisper`。

从源码启动当前目录：

```bash
npm run tui:dev
```

指定工作目录：

```bash
npm run tui:dev -- --cwd /path/to/project
```

构建包含 `pisper`、SEA sidecar 和 runtime 的完整目录：

```bash
npm run sidecar:sea
npm run tui:package
```

将生成的 `release/tui/pisper-<version>-<platform>-<arch>/` 加入 `PATH` 后，可以在任意项目目录直接启动：

```bash
pisper
```

普通启动始终创建空会话，不会自动载入历史。只有显式使用 `resume` 才恢复当前工作目录最近的会话：

```bash
pisper resume
pisper resume --cwd /path/to/project
```

诊断 sidecar、鉴权与当前能力目录：

```bash
pisper doctor
```

#### 调整当前会话权限

输入 `/mode` 查看当前执行模式。使用以下命令切换当前会话：

| 命令 | 行为 |
| :--- | :--- |
| `/mode read-only` | 只开放低风险分析工具，不允许修改项目。 |
| `/mode workspace` | 允许在当前工作目录内修改；Shell 运行在本地沙箱中，越界操作仍需审批。 |
| `/mode full-access` | 允许访问工作目录外的文件和网络，Shell 不再受工作区沙箱限制。仅在明确需要时使用。 |

Windows 首次切换到 `workspace` 时可能显示一次 UAC，用于创建低权限沙箱账户和网络隔离规则。安装失败或被取消时，TUI 不会静默切换权限。

#### 调用 Tool

1. 在 composer 中输入 `/`。
2. 选择前缀为 `T` 的 Tool。
3. 在命令后写清目标或参数，然后按 `Enter`。

示例：

```text
/read README.md
/bash npm test
/web_search Pisper latest release
/mcp_pencil_get_editor_state_f9837b9b 读取当前 Pencil 画布状态
```

TUI 会把选中的 Tool 名作为结构化请求交给 runtime，确保对应 Schema 在当前回合可用；随后仍由 Agent 组织参数并执行。Slash 选择不会绕过 Tool 开关、当前执行模式、工作区边界或 runtime 审批。被禁用或不可用的 Tool 不会出现在列表中。

#### 调用 Skill

输入 `/` 后选择前缀为 `S` 的 Skill，再在命令后补充本次任务。例如已启用的 Skill 命令为 `/skill:docs-search` 时：

```text
/skill:docs-search 查找 Tauri updater 的签名要求
```

Skill 名称来自当前 runtime，实际命令以 Slash 列表为准。只有已启用且允许模型调用的 Skill 才会显示；选中后，runtime 按需加载对应 `SKILL.md`、脚本和参考资料。Skill 内部调用 Tool 时仍服从当前 `/mode`、沙箱和审批策略。

#### TUI 内置命令

| 命令 | 作用 |
| :--- | :--- |
| `/new` | 新建会话。 |
| `/sessions` | 切换历史会话。 |
| `/events` | 打开当前 TUI 进程的事件账本。 |
| `/chat` | 返回消息流。 |
| `/model` | 显示当前模型。 |
| `/mode` | 显示当前执行模式及可用参数。 |
| `/quit` | 退出 TUI 并关闭它启动的 sidecar。 |

运行中按 `Ctrl+C` 会终止当前 Agent；空闲时按 `Ctrl+C` 会退出。runtime 请求 Tool 审批时，按 `Y` 同意，按 `N` 或 `Esc` 拒绝。

更完整的开发与发行布局说明见 [`src-tui/README.md`](./src-tui/README.md)。

### 从源码运行

需要 Node.js 20+、npm，以及至少一个模型 Provider 与 API Key。

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm install
npm run dev
```

Web 开发地址默认为 `http://127.0.0.1:5173`。生产模式运行：

```bash
npm run build
npm start
```

桌面开发与打包：

```bash
npm run desktop:dev
npm run desktop:pack
```

配置、会话与记忆默认保存在 `~/.pisper/agent`，可通过 `PISPER_AGENT_DIR` 修改。从 Vesper 升级时，首次启动会自动把旧的 `~/.vesper` 迁移到 `~/.pisper`（新目录已有数据则不覆盖，旧目录保留作备份）。

<a id="development"></a>

## 开发

**技术栈：** TypeScript、React 19、Vite、Tailwind CSS、shadcn/ui、Dockview、React Flow、Zustand、i18next、Tauri 2、Rust、Node SEA 与 Pi Coding Agent。

```bash
npm run check
npm test
npm run build
```

欢迎提交 [Issue](https://github.com/ling-kong-ran/pisper/issues) 与 [Pull Request](https://github.com/ling-kong-ran/pisper/pulls)。请勿提交 API Key、机器人凭据，或 `~/.pisper/agent` 中的个人数据。

<a id="sponsors"></a>

## 赞助

感谢以下合作伙伴对 Pisper 社区的支持：

- [Matrix](https://matrix.000328.xyz/sign-up?aff=ZPeH)

Matrix 链接包含推广参数；通过该链接注册可能为 Pisper 项目带来推广收益。Pisper 的赞助内容不会使用会话、工作区、Provider、模型或 API 配置进行定向，也不会向赞助商发送这些数据。

客户端赞助位的公开配置维护在 [`docs/sponsors.json`](./docs/sponsors.json)。

<a id="license"></a>

## 许可

Pisper 采用 [MIT License](./LICENSE)。第三方依赖与外部资源仍遵循各自许可证与权利声明；Petdex 社区宠物资源归相应作者或权利人所有，不属于 Pisper 源码或发行包。

## 致谢

Pisper 基于 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 构建，并受益于 Node.js、React、TypeScript、Vite、Tailwind CSS、shadcn/ui、React Router、Zustand、React Flow、i18next 等开源项目。

感谢 [Petdex](https://petdex.dev) 与 [`crafter-station/petdex`](https://github.com/crafter-station/petdex) 提供 MIT 许可的兼容格式与目录实现参考。

贡献者：

- [@mik-myp](https://github.com/mik-myp) — 前端 TypeScript 架构、shadcn/ui / AI Elements、Zustand 与 i18n 重构（[#1](https://github.com/ling-kong-ran/pisper/pull/1)）

<p align="right"><a href="#top">返回顶部 ↑</a></p>
