<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper 项目标志" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>Pi 驱动的多 Agent 工作台</strong></p>

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

<a id="sponsors"></a>

## ❤️ 赞助

感谢以下合作伙伴对 Pisper 社区的支持。如果你也希望出现在这里，欢迎通过 [Issue](https://github.com/ling-kong-ran/pisper/issues) 联系我们。

<details open>
<summary>查看赞助商</summary>

<table>
<tr>
<td width="180" align="center">
  <a href="https://matrix.000328.xyz/sign-up?aff=ZPeH"><strong>Matrix</strong></a>
</td>
<td>
感谢 <a href="https://matrix.000328.xyz/sign-up?aff=ZPeH">Matrix</a> 对 Pisper 社区的支持。通过<a href="https://matrix.000328.xyz/sign-up?aff=ZPeH">此链接</a>注册，可能为 Pisper 项目带来推广收益。
</td>
</tr>
</table>

> 赞助链接包含推广参数。Pisper 的赞助内容不会使用会话、工作区、Provider、模型或 API 配置进行定向，也不会向赞助商发送这些数据。客户端赞助位的公开配置维护在 [`docs/sponsors.json`](./docs/sponsors.json)。

</details>

<p align="center">
  <a href="#overview">简介</a> ·
  <a href="https://ling-kong-ran.github.io/pisper/">项目主页</a> ·
  <a href="#features">功能</a> ·
  <a href="#data-safety">数据安全</a> ·
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

Pisper 是基于 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 的桌面与终端客户端，支持多会话、工具、Skills、MCP、自动化和会话级权限。

产品介绍与界面预览：[Pisper 项目主页](https://ling-kong-ran.github.io/pisper/)。

### 三步上手

1. [下载桌面版](https://github.com/ling-kong-ran/pisper/releases/latest)（Windows / macOS / Linux）
2. 配置任意一个模型 Provider 与 API Key
3. 新建会话，开始并行工作

<a id="features"></a>

## 功能

- **多会话**：独立模型、上下文、工作目录和权限，支持 Dock 分屏与布局恢复。
- **Tools、Skills 与 MCP**：统一管理能力和调用权限。
- **Subagent**：在隔离上下文中执行临时任务，完成后将结果返回父会话。
- **记忆与多模态**：检索项目记忆，处理图片、文档和代码。
- **自动化与渠道**：定时任务、可视化工作流、飞书和个人微信。
- **桌面与终端**：提供 Tauri 桌面端和 Ratatui TUI。
- **权限控制**：支持 `只读 / 完全访问` 两种执行模式，并提供单次审批和凭据隔离。

<a id="data-safety"></a>

## 数据安全与隐私

Pisper 是本地优先应用，不提供用于托管或中转会话的 Pisper 云服务。会话、记忆、工作流、日程、用量和连接配置默认保存在本机 `~/.pisper/agent`；可通过 `PISPER_AGENT_DIR` 更改位置。

- **本地服务保护**：桌面 Runtime 默认只监听 `127.0.0.1`，使用随机启动 Token、`HttpOnly` / `SameSite=Strict` Cookie 和写请求 Origin 校验；Pi 遥测默认关闭。
- **已知敏感信息脱敏**：API Key、Bearer/JWT、常见访问令牌、私钥和数据库连接串等已知格式，会在记忆落盘、记忆二次模型提取，以及错误、MCP 配置和审批摘要等展示边界进入前被替换。脱敏后的内容使用 `[REDACTED SECRET]` 或 `***` 标记。
- **凭据隔离**：Provider 凭据、MCP Header/环境变量和渠道凭据保存在本机配置中，不通过普通配置接口回显给 Agent；宿主 Shell 工具也会移除常见凭据环境变量和 Shell 注入变量。
- **记忆可控**：自动推断的会话记忆先进入待确认区，未确认前不参与召回；用户可以拒绝候选、删除单条记忆或删除会话，删除操作会清除对应正文与检索数据。
- **外发边界明确**：普通 Prompt、附件、工具结果不会被全局改写或匿名化。调用模型时，必要上下文会直接发送到你配置的 Provider；启用远程 MCP、Web 搜索、图像/视频生成或消息渠道时，相应请求也会发送给该第三方。记忆提取和语义摘要如使用模型，会先执行上述已知格式脱敏，再调用当前 Provider。

> **重要边界：** 脱敏是降低常见凭据意外泄露风险的纵深防护，不是完整的 DLP、沙箱或端到端加密，也无法识别所有自由文本中的敏感数据。Provider 和渠道凭据目前保存在本机 Agent 数据目录，而不是操作系统钥匙串；请保护该目录与备份，并在发送前检查 Prompt、文件和工具参数，同时遵守所选第三方的隐私政策。

<a id="desktop-pet"></a>

## 桌面宠物

Pisper 支持 [Petdex](https://petdex.dev) 兼容宠物，可在 **设置 → 桌面宠物** 中安装和管理。详见 [`docs/petdex-integration.md`](./docs/petdex-integration.md)。

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

### 分块更新

桌面端的 **设置 → 更新** 是统一更新入口。点击一次 **检查更新** 会同时检查 Desktop、TUI 客户端和 Runtime；只有发现新版本的组件才会显示安装操作，因此小范围更新不必重新下载完整桌面安装包。三个组件使用独立的签名 Release 通道，Runtime 更新在重启应用后生效；独立组件不可用或启动失败时，Pisper 会继续使用桌面安装包内置的版本。

<a id="tui"></a>

### 终端客户端（TUI）

使用 Node.js 20+ 时，也可通过 npm 独立安装。`pisper-cli` 会下载并验证官方签名的 TUI 与 Runtime 组件，npm registry 中不发布 Pisper Runtime：

```bash
npm install -g pisper-cli
pisper
```

安装桌面版后，也可以在 **设置 → 终端** 中安装 `pisper` 命令。首次安装由你主动确认；之后桌面应用更新并重启时，Pisper 会自动刷新这份已托管的终端客户端：

```bash
pisper          # 新建会话
pisper resume   # 从所有工作目录的交互列表中恢复会话
pisper doctor   # 诊断运行环境
```

安装、命令、快捷键、附件、执行模式和审批说明见 **[Pisper TUI 使用指南](./src-tui/README.md)**。

### 从源码运行

需要 Node.js 20+、npm，以及至少一个模型 Provider 与 API Key。

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm install
npm run dev
```

桌面开发与打包：

```bash
npm run desktop:webview:dev
npm run desktop:webview:build
```

数据默认保存在 `~/.pisper/agent`，可通过 `PISPER_AGENT_DIR` 修改。

<a id="development"></a>

## 开发

主要技术栈：React、TypeScript、Tauri、Rust、Node SEA 与 Pi Coding Agent。

```bash
npm run check
npm test
npm run build
```

欢迎提交 [Issue](https://github.com/ling-kong-ran/pisper/issues) 与 [Pull Request](https://github.com/ling-kong-ran/pisper/pulls)。请勿提交 API Key、机器人凭据，或 `~/.pisper/agent` 中的个人数据。

<a id="license"></a>

## 许可

Pisper 采用 [MIT License](./LICENSE)。第三方依赖和社区资源遵循各自许可证。

## 致谢

感谢 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent)、[Petdex](https://petdex.dev) 及本项目使用的开源软件。

贡献者：

- [@mik-myp](https://github.com/mik-myp) — 前端 TypeScript 架构、shadcn/ui / AI Elements、Zustand 与 i18n 重构（[#1](https://github.com/ling-kong-ran/pisper/pull/1)）

<p align="right"><a href="#top">返回顶部 ↑</a></p>
