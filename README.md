<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/vesper-logo.svg" width="112" alt="Vesper 项目标志" />
</p>

<h1 align="center">Vesper</h1>

<p align="center"><strong>暮色落下，灵感仍醒着。</strong></p>
<p align="center">本地优先的多 Agent 工作台，在一个可停靠工作区中连接会话、工具、记忆与自动化。</p>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js" />
  <img src="https://img.shields.io/badge/TypeScript-17141F?style=flat-square&logo=typescript&logoColor=F59E0B" alt="TypeScript" />
  <img src="https://img.shields.io/badge/React-17141F?style=flat-square&logo=react&logoColor=F59E0B" alt="React" />
  <img src="https://img.shields.io/badge/Electron-17141F?style=flat-square&logo=electron&logoColor=F59E0B" alt="Electron" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/vesper/releases/latest">
    <img src="https://img.shields.io/badge/下载桌面版-Windows%20%7C%20macOS%20%7C%20Linux-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="下载 Vesper 桌面版" />
  </a>
</p>

<p align="center">
  <a href="#overview">简介</a> ·
  <a href="#preview">界面</a> ·
  <a href="#features">功能</a> ·
  <a href="#desktop-pet">桌面宠物</a> ·
  <a href="#install">安装</a> ·
  <a href="#development">开发</a> ·
  <a href="#license">许可</a>
</p>

---

<a id="overview"></a>

## 简介

Vesper 是一款**本地优先的多 Agent 工作台**。每个会话拥有独立的模型、上下文、工作目录和执行权限，并可通过标签、左右分屏与拖拽停靠组合成 IDE 式工作区。

- 多会话并行运行，布局自动恢复。
- 动态 Prompt 只加载当前任务需要的工具。
- 记忆、定时任务、工作流、MCP 与外部渠道统一管理。
- 敏感操作受执行模式、工作区、沙箱和审批约束。

<a id="preview"></a>

## 界面

<p align="center">
  <img src="./docs/shots/welcome-dark.png" alt="Vesper 暗色主题欢迎页" />
</p>

<table>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/chat-grid.png" alt="Vesper 多会话 Dock 分屏工作区" />
      <br /><sub><strong>Dock 工作区</strong> · 标签、分屏与拖拽停靠</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/memory.png" alt="Vesper 星忆视图" />
      <br /><sub><strong>星忆</strong> · 保存可搜索的本地记忆</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="./docs/shots/workflow-builder.png" alt="Vesper 工作流编辑器" />
      <br /><sub><strong>工作流</strong> · 编排可复用自动化</sub>
    </td>
    <td width="50%" align="center">
      <img src="./docs/shots/channels.png" alt="Vesper 渠道视图" />
      <br /><sub><strong>双向渠道</strong> · 连接飞书与个人微信</sub>
    </td>
  </tr>
</table>

<a id="features"></a>

## 功能

| 模块 | 能力 |
| :--- | :--- |
| **多会话对话** | 独立 Agent 会话、模型与权限；支持标签分组、左右拆分、拖拽停靠和布局恢复。 |
| **Agent Runtime** | 基于 Pi Coding Agent，支持工具活动、Goal、Skills 与隔离 Subagent。 |
| **动态 Prompt** | 保持稳定的轻量基础 Prompt，只按明确意图追加低频工具 Schema。 |
| **工具与 MCP** | 统一管理内置工具、插件和 MCP 服务，不向前端暴露凭据。 |
| **星忆** | 使用本地 SQLite 保存并搜索偏好、事实、决策与任务。 |
| **多模态** | 阅读图片、文档和代码，并通过已配置模型生成或编辑视觉内容。 |
| **自动化** | 定时任务与可视化工作流，支持重试、失败策略、历史和通知。 |
| **双向渠道** | 连接飞书与个人微信，支持独立模型、工作目录和附件。 |
| **Web 预览** | 在 Dock 面板中预览外部网页，并保留系统浏览器回退。 |
| **桌面应用** | Electron 单实例应用，支持应用内更新日志和 GitHub Releases 更新。 |
| **安全边界** | 会话级 `只读 / 工作区 / 完全访问`、单次审批、凭据脱敏和本地数据隔离。 |

### 轻量 Prompt

普通编码会话默认只加载高频工具。Web Search、浏览器、视觉生成、星忆、MCP 和 Subagent 等能力仅在明确需要时追加，并在下一次普通请求中回到轻量基线。动态激活不会绕过工具开关、执行模式、沙箱或审批策略。

> 当前基准下，固定 Prompt 约为 **2,979 tokens**，相比全量注入减少约 **58.7%**。实际计费与缓存收益因模型和服务商而异。

> **沙箱说明：** 默认“工作区”模式使用 [Anthropic Sandbox Runtime](https://github.com/anthropic-experimental/sandbox-runtime) 限制 Shell 写入、凭据和网络访问。Windows 首次启用需要一次 UAC；初始化失败时 Vesper 会阻止执行，不会静默回退到完整权限。该运行时仍处于 Beta Research Preview。

<a id="desktop-pet"></a>

## 桌面宠物

Vesper 原生支持 [Petdex](https://petdex.dev) 兼容宠物，可在 **设置 → 桌面宠物** 中搜索、安装、切换和启停，无需安装 Petdex CLI 或独立进程。

- Agent 的等待、思考、工具执行、完成与失败状态会映射为宠物动画。
- Electron 使用独立透明窗口，支持拖动、置顶、多显示器位置记忆和 `20%–100%` 透明度；隐藏主窗口后仍可显示。
- Web 版使用页面内可拖动悬浮层，关闭页面后随之退出。
- 资源仅从 HTTPS 白名单下载，并验证大小、格式、精灵网格和路径边界；社区宠物不会预打包。

详见 [`docs/petdex-integration.md`](./docs/petdex-integration.md)。

<a id="install"></a>

## 安装

### 桌面版

前往 [GitHub Releases](https://github.com/ling-kong-ran/vesper/releases/latest) 下载 Windows、macOS 或 Linux 安装包，无需安装 Node.js。

#### macOS 无法打开

Vesper 当前未经过 Apple 公证。请确认应用来自官方 Releases，并优先在 **系统设置 → 隐私与安全性** 中选择 **仍要打开**。如果没有该选项，可执行：

```bash
sudo xattr -rd com.apple.quarantine /Applications/Vesper.app
```

仅对从 Vesper 官方 Releases 下载并放入 `/Applications` 的应用使用此命令。

#### Linux AppImage 无法启动

```bash
chmod +x Vesper-*-linux-x86_64.AppImage
./Vesper-*-linux-x86_64.AppImage
```

缺少 FUSE 时安装 `libfuse2` 或 `libfuse2t64`，也可改用 `.deb`：

```bash
sudo apt install ./Vesper-*-linux-amd64.deb
```

### 从源码运行

需要 Node.js 20+、npm，以及至少一个模型 Provider 和 API Key。

```bash
git clone https://github.com/ling-kong-ran/vesper.git
cd vesper
npm install
npm run dev
```

Web 开发地址默认为 `http://127.0.0.1:5173`。生产模式：

```bash
npm run build
npm start
```

桌面开发与本地打包：

```bash
npm run desktop:dev
npm run desktop:pack
```

本地配置、会话与记忆默认保存在 `~/.vesper/agent`，可通过 `VESPER_AGENT_DIR` 修改。

<a id="development"></a>

## 开发

**技术栈：** TypeScript、React 19、Vite、Tailwind CSS、shadcn/ui、Dockview、React Flow、Zustand、i18next、Node.js、Electron 与 Pi Coding Agent。

```bash
npm run check
npm test
npm run build
```

欢迎提交 [Issue](https://github.com/ling-kong-ran/vesper/issues) 和 [Pull Request](https://github.com/ling-kong-ran/vesper/pulls)。请勿提交 API Key、机器人凭据或 `~/.vesper/agent` 中的本地数据。

<a id="license"></a>

## 许可

Vesper 采用 [MIT License](./LICENSE)。第三方依赖与外部资源继续适用各自的许可证和权利声明；Petdex 社区宠物资源归相应作者或权利人所有，不属于 Vesper 源码或发行包。

## 致谢

Vesper 基于 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 构建，并受益于 Node.js、React、TypeScript、Vite、Tailwind CSS、shadcn/ui、React Router、Zustand、React Flow、i18next 等开源项目。

感谢 [Petdex](https://petdex.dev) 与 [`crafter-station/petdex`](https://github.com/crafter-station/petdex) 提供 MIT 许可的兼容格式和目录实现参考。

贡献者：

- [@mik-myp](https://github.com/mik-myp) — 前端 TypeScript 架构、shadcn/ui / AI Elements、Zustand 与 i18n 重构（[#1](https://github.com/ling-kong-ran/vesper/pull/1)）

<p align="right"><a href="#top">返回顶部 ↑</a></p>
