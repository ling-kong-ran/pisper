<p align="right"><strong>简体中文</strong> · <a href="./README.en.md">English</a></p>

<a id="top"></a>

<p align="center">
  <img src="docs/brand/banner.svg" width="880" alt="Pisper — 分身，不分心" />
</p>

<p align="center">
  跨桌面、终端与手机的多 Agent 应用：像管理代码分支一样管理 Agent 的思路，从任意已完成 Turn 长出分支，并行推进。
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases"><img src="https://img.shields.io/github/v/release/ling-kong-ran/pisper?style=flat-square&label=Release" alt="Release" /></a>
  <a href="https://github.com/ling-kong-ran/pisper/stargazers"><img src="https://img.shields.io/github/stars/ling-kong-ran/pisper?style=flat-square&label=Stars" alt="GitHub Stars" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/License-MIT-F59E0B?style=flat-square" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/Node.js-20%2B-17141F?style=flat-square&logo=nodedotjs&logoColor=F59E0B" alt="Node.js 20+" />
  <img src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-17141F?style=flat-square" alt="支持平台" />
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest">
    <img src="https://img.shields.io/badge/下载桌面版-自带%20TUI%20与%20Runtime-F59E0B?style=for-the-badge&logo=github&logoColor=17141F" alt="下载 Pisper 桌面版" />
  </a>
</p>

<p align="center">
  <a href="https://ling-kong-ran.github.io/pisper/#mobile">移动端下载</a> ·
  <a href="https://github.com/ling-kong-ran/pisper/releases?q=app-v">App Releases</a>
</p>

<p align="center">
  <a href="https://ling-kong-ran.github.io/pisper/">项目主页</a> ·
  <a href="https://ling-kong-ran.github.io/pisper/guide.html">使用教程</a> ·
  <a href="#quickstart">三分钟上手</a> ·
  <a href="#features">能力地图</a> ·
  <a href="#data-safety">数据安全</a> ·
  <a href="./README.en.md">English</a>
</p>

<a id="why"></a>

## ✨ 为什么是 Pisper

- **对话也能开分支。** 在任意已完成 Turn 衍生新会话，继承上下文，源会话一字不改；稳定 Turn 标签把关键节点钉成可检索的锚点 —— 像 Git，但给 Agent 用。
- **多 Agent 真并行。** 每个会话独享模型、上下文、工作目录与权限；拖动标签四面分屏，进度同屏可见。
- **工具冷热分明。** 核心工具常驻上下文；插件、MCP 与技能经 discover/call 网关按需激活、用完即退 —— 能力再丰富，也不把上下文塞成杂物间。
- **前缀稳，缓存才热。** 工具定义稳定化排序、提示词形态哈希诊断，尽量吃满 Provider 的 prompt cache —— 长会话更快、更省。
- **缺什么能力，直接说。** Pisper 会自己编写、校验并安装本地插件，下一轮对话就能调用。
- **手机能独立，也能接桌面。** Android / iOS App 内置与桌面同源的 Node/Pisper Runtime、标准会话和 React 界面，并按设备实际能力关闭不可用入口；也可扫码连接桌面，优先 LAN、离开局域网后自动回退 Iroh P2P。远程链路继续使用 TLS 指纹与设备 Bearer 令牌。
- **数据默认不出机。** Runtime 默认只听 127.0.0.1，敏感格式自动脱敏，记忆先审后用 —— 你的上下文，你说了算。

<a id="features"></a>

## 🗺️ 能力地图

| 🌿 并行与分叉 | 🧩 能力扩展 |
| --- | --- |
| 并行会话分屏 · 追忆分支树 · 稳定 Turn 标签 · Ctrl+K 跨会话直达 · 会话级模型/目录/权限 | 本地插件自动生成 · MCP 服务 · 技能中心 · 多 Provider 模型配置 |
| **⚡ 自动化与通知** | **🖥️ 终端与桌面一体** |
| 可视化工作流 · 定时任务 · 飞书 / 个人微信双向渠道 · 星忆项目记忆 · Git 与 SVN 工作区 | Ratatui TUI 与桌面共用 Runtime · Android / iOS 同源本机 Runtime 或桌面连接 · 桌面宠物（Petdex）· Desktop / TUI / Runtime / App 独立更新 |

<a id="pi-runtime"></a>

## 🧠 基于 Pi Coding Agent 深度构建

Pisper 以 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 作为底层 Agent Runtime。在 Pi 提供的模型接入与工具执行基础上，Pisper 围绕真实的多 Agent 工作持续做深度产品化与优化：

- **运行时与会话编排**：把独立会话、并行执行、Turn 分支、工作目录与权限策略组织成可持续运行的多 Agent 系统。
- **上下文与性能**：通过工具冷热分层、discover/call 按需加载、稳定工具定义与提示词形态诊断，减少上下文占用并提高 Provider prompt cache 命中率。
- **完整产品层**：提供 Desktop、Ratatui TUI 与移动端体验；手机本机模式运行同一套 Node/Pisper Agent、Provider、会话、HTTP/SSE 与 React，并按 embedded Node 的实际能力清单关闭当前设备无法承载的入口。

## 📸 界面预览

<table>
  <tr>
    <td><a href="docs/shots/chat-grid.png"><img src="docs/shots/web/chat-grid.webp" alt="并行会话分屏" /></a></td>
    <td><a href="docs/shots/session-tree.png"><img src="docs/shots/web/session-tree.webp" alt="追忆分支视图" /></a></td>
  </tr>
  <tr>
    <td align="center">并行会话：拖标签，四面分屏</td>
    <td align="center">追忆：在任意已完成 Turn 接回原分支</td>
  </tr>
  <tr>
    <td><a href="docs/shots/workflow-builder.png"><img src="docs/shots/web/workflow-builder.webp" alt="可视化工作流编辑器" /></a></td>
    <td><a href="docs/shots/cli-chat.png"><img src="docs/shots/web/cli-chat.webp" alt="TUI Chat 界面" /></a></td>
  </tr>
  <tr>
    <td align="center">工作流：把重复工作连成流程</td>
    <td align="center">TUI：离开桌面，上下文不走</td>
  </tr>
</table>

> 更多界面与交互演示见 **[项目主页](https://ling-kong-ran.github.io/pisper/)**。

<a id="quickstart"></a>

## 🚀 三分钟上手

> 需要更详细的图文说明？见 **[使用教程](https://ling-kong-ran.github.io/pisper/guide.html)**。

### 方式一：桌面版（推荐）

从 [Releases](https://github.com/ling-kong-ran/pisper/releases/latest) 下载对应平台安装包，**自带 TUI 与 Runtime，无需安装 Node.js**。

<details>
<summary>macOS 提示「无法打开」？</summary>

Pisper 当前尚未经过 Apple 公证。请确认安装包来自官方 Releases，并在 **系统设置 → 隐私与安全性** 中选择 **仍要打开**。若没有该选项：

```bash
sudo xattr -rd com.apple.quarantine /Applications/Pisper.app
```

请仅对从官方 Releases 下载并放入 `/Applications` 的应用使用此命令。

</details>

<details>
<summary>Linux AppImage 无法启动？</summary>

```bash
chmod +x Pisper_*_linux_x86_64.AppImage
./Pisper_*_linux_x86_64.AppImage
```

缺少 FUSE 时安装 `libfuse2` 或 `libfuse2t64`，也可改用 `.deb`：

```bash
sudo apt install ./Pisper-*-linux-amd64.deb
```

</details>

### 方式二：移动端 App

Android / iOS App 首次启动会直接进入内置的本机 Runtime；连接桌面端是正常 Pisper 界面中的可选设置，不会阻塞首次使用。项目主页会从 `docs/latest-app.json` 解析最新 App 下载地址，但页面不显示或写死具体版本号：

| 平台 | 下载 | 安装状态 |
| --- | --- | --- |
| Android | [下载已签名 APK](https://ling-kong-ran.github.io/pisper/#mobile) | 已签名 arm64 APK，可直接安装；首次侧载时按系统提示允许安装未知来源应用。 |
| iOS | [下载未签名 IPA](https://ling-kong-ran.github.io/pisper/#mobile) | **未签名**，不能直接安装；需使用 AltStore、Sideloadly 或自己的 Apple 开发者账号重签。 |

**本机运行**

<details>
<summary>展开步骤</summary>

1. 首次启动等待内置 Runtime 就绪，App 会直接打开正常 Pisper 界面；从远程桌面返回时可进入 **设置 → 服务器 → 在本机运行**。
2. 配置 Provider 和模型；会话、Provider 配置与工作区只保存在手机 App 私有目录。
3. 本机 Runtime 仅监听随机回环端口。Android/iOS 会按 embedded Node 的实际模块清单隐藏 Shell、MCP、工作流等不可用能力。

</details>

**连接桌面端**

<details>
<summary>展开步骤</summary>

1. 建议首次配对时让手机与电脑接入同一局域网，在桌面端打开 **设置 → 远程访问** 并开启远程访问。
2. 在手机 **设置 → 服务器 → 添加服务器** 中允许本地网络访问，选择自动发现的桌面并发起申请；桌面用户明确批准后才会签发设备令牌。
3. 异地连接或局域网发现失败时，在桌面生成一次性二维码，再由手机扫描二维码或手动输入。二维码截图可以发到异地手机；所有路径都校验 TLS 指纹并使用设备 Bearer 令牌，连接后优先 LAN、不可达时回退 Iroh P2P。

</details>

手机会记住当前使用的本机或远程 Runtime。完整流程、能力边界、安全模型与排障见 **[移动端使用指南](./docs/mobile.md)** 和 **[本机 Runtime 设计](./docs/mobile-local-runtime.md)**。

### 方式三：npm（Node.js 20+）

```bash
npm i -g pisper
pisper web   # 打开 Web 前端与本机配置页
```

首次进入使用 `/provider` 选择 Provider 并配置 API Key。可用命令及参数见 **[TUI 命令参考](./src-tui/README.md)**。

### 方式四：从源码运行

<details>
<summary>展开</summary>

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

</details>

数据默认保存在 `~/.pisper/agent`，可通过 `PISPER_AGENT_DIR` 修改。

<a id="data-safety"></a>

## 🔒 数据安全

Pisper 没有「我们的云」。日常数据默认由本机 Runtime 持有，只有你配置并实际调用的 Provider、MCP、搜索或渠道，才会收到完成请求所需的内容。

- **默认只听本机**：桌面 Runtime 常规入口绑定 127.0.0.1；手机本机 Runtime 只监听随机回环端口。只有显式开启桌面远程访问后，才会额外启动 LAN HTTPS 与 Iroh P2P endpoint。Iroh 只承载原始加密字节，上层仍由 TLS 指纹和设备 Bearer 令牌保护。Pi 遥测默认关闭。
- **敏感先脱敏、凭据留在沙箱**：常见 API Key、Bearer/JWT、私钥与连接串，在记忆落盘与摘要展示前被替换。手机本机 Provider 凭据与远程设备令牌当前保存在 App 私有目录，依赖系统沙箱与文件权限保护，尚未迁入 Android Keystore 或 iOS Keychain。
- **权限有边界**：需审批、工作区写入、完全访问三档；凭据不经普通接口回显给 Agent，宿主 Shell 会移除常见凭据环境变量。
- **记忆需确认**：自动推断的记忆先进入待确认区，你点头之前不参与召回。

> 边界说明：脱敏只识别常见敏感格式，不是完整 DLP、沙箱或端到端加密。桌面 Runtime 的 Provider 凭据仍位于本机 Agent 数据目录，请保护该目录和备份；手机安全存储也不能替代设备锁屏、系统更新与可信侧载来源。完整说明见[项目主页数据安全部分](https://ling-kong-ran.github.io/pisper/#safety)。

## 🧩 组件与独立更新

Desktop、TUI、Runtime 与移动 App 各自独立版本、独立签名、独立更新，失败自动回退到内置版本。桌面端提供统一组件检查入口；移动 App 使用独立发布清单。

## 📚 文档

**新手从这里开始** → **[使用教程](https://ling-kong-ran.github.io/pisper/guide.html)**：安装、模型配置、并行会话、分支、工作流、终端与移动端的完整说明。

| 文档 | 内容 |
| --- | --- |
| [使用教程](https://ling-kong-ran.github.io/pisper/guide.html) | 从安装到进阶的完整上手路径 |
| [项目主页](https://ling-kong-ran.github.io/pisper/) | 产品介绍与界面演示 |
| [TUI 命令参考](./src-tui/README.md) | CLI、Slash command 与参数说明 |
| [移动端使用指南](./docs/mobile.md) | 安装、本机运行、桌面配对、安全模型与排障 |
| [移动端本机 Runtime](./docs/mobile-local-runtime.md) | 统一 Node 架构、能力清单与供应链 |
| [插件开发指南](./docs/plugin-authoring.md) | 编写与发布插件；另见 [本地插件指南](./docs/local-plugins.md) |
| [桌面宠物（Petdex）](./docs/petdex-integration.md) | 桌面宠物集成说明 |

<a id="development"></a>

## 🛠️ 开发

底层 Agent Runtime 基于 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent) 深度集成；产品层主要使用 React、TypeScript、Tauri、Rust 与 Node SEA。

```bash
npm run check   # typecheck + lint + i18n + format + 启动检查
npm test        # runtime 测试
npm run build   # 生产构建
```

欢迎提交 [Issue](https://github.com/ling-kong-ran/pisper/issues) 与 [Pull Request](https://github.com/ling-kong-ran/pisper/pulls)。请勿提交 API Key、机器人凭据，或 `~/.pisper/agent` 中的个人数据。

## 🙏 致谢

感谢 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent)、[Petdex](https://petdex.dev) 及本项目使用的开源软件。

贡献者：

- [@mik-myp](https://github.com/mik-myp) — 前端 TypeScript 架构、shadcn/ui / AI Elements、Zustand 与 i18n 重构（[#1](https://github.com/ling-kong-ran/pisper/pull/1)）

<a id="sponsors"></a>

## ❤️ 赞助

<details>
<summary>查看赞助商</summary>

<table>
<tr>
<td width="180" align="center">
  <a href="https://matrix.000328.xyz/register?aff=ZPEH"><strong>Matrix</strong></a>
</td>
<td>
感谢 <a href="https://matrix.000328.xyz/register?aff=ZPEH">Matrix</a> 对 Pisper 社区的支持。通过<a href="https://matrix.000328.xyz/register?aff=ZPEH">此链接</a>注册，可能为 Pisper 项目带来推广收益。
</td>
</tr>
</table>

> 赞助链接包含推广参数。Pisper 的赞助内容不会使用会话、工作区、Provider、模型或 API 配置进行定向，也不会向赞助商发送这些数据。客户端赞助位的公开配置维护在 [`docs/sponsors.json`](./docs/sponsors.json)。

</details>

如果你也希望出现在这里，欢迎通过 [Issue](https://github.com/ling-kong-ran/pisper/issues) 联系我们。

---

<p align="center">
  <strong>如果 Pisper 对你有用，点一颗 ⭐ —— 这是让项目继续生长的最大动力。</strong><br />
  <sub>也欢迎分享给每一个把 Coding Agent 当生产力工具的人。</sub>
</p>

<p align="center">
  <a href="./LICENSE">MIT License</a> · © Pisper Contributors ·
  <a href="#top">返回顶部 ↑</a>
</p>
