<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="112" alt="Pisper 项目标志" />
</p>

<h1 align="center">Pisper</h1>

<p align="center"><strong>Pi 驱动的本地 Agent 应用</strong></p>

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

产品介绍、界面预览与上手入口统一维护在 [Pisper 项目主页](https://ling-kong-ran.github.io/pisper/)。

<a id="features"></a>

## 功能

完整能力说明与界面演示见[项目主页的产品与能力部分](https://ling-kong-ran.github.io/pisper/#product)。插件使用与开发细节见[本地插件指南](./docs/local-plugins.md)和[插件开发指南](./docs/plugin-authoring.md)。

<a id="data-safety"></a>

## 数据安全与隐私

本地数据边界、第三方数据流向、凭据脱敏范围与限制统一说明在[项目主页的数据安全部分](https://ling-kong-ran.github.io/pisper/#safety)。

<a id="desktop-pet"></a>

## 桌面宠物

Pisper 支持 [Petdex](https://petdex.dev) 兼容宠物，可在 **设置 → 桌面宠物** 中安装和管理。详见 [`docs/petdex-integration.md`](./docs/petdex-integration.md)。

<a id="install"></a>

## 安装

### 桌面版（推荐）

桌面版下载、支持平台和基础安装说明见 [Pisper 项目主页](https://ling-kong-ran.github.io/pisper/)。

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

Desktop、TUI 与 Runtime 的独立更新方式见[项目主页](https://ling-kong-ran.github.io/pisper/#updates)。

<a id="tui"></a>

### 终端客户端（TUI）

终端安装入口见[项目主页](https://ling-kong-ran.github.io/pisper/#terminal)；完整安装、更新、命令、快捷键、附件、Provider 配置、执行模式与审批说明见 **[Pisper TUI 使用指南](./src-tui/README.md)**。

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
