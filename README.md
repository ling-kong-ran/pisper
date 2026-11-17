<p align="center">
  <img src="docs/brand/pisper-logo.svg" width="72" alt="" />
</p>

<h1 align="center">Pisper</h1>

<p align="center">
  <b>想法，不必排队。</b><br />
  几个代理同时干活，谁在等你批准、谁还在跑、谁已经做完，一眼看清。<br />
  桌面、终端和手机共用同一个内核——换设备，现场不换。
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest"><img src="https://img.shields.io/github/v/release/ling-kong-ran/pisper?style=flat-square&label=release&color=8B5CF6" alt="最新版本" /></a>
  <a href="https://www.npmjs.com/package/pisper"><img src="https://img.shields.io/npm/v/pisper?style=flat-square&color=8B5CF6" alt="npm" /></a>
  <a href="https://github.com/ling-kong-ran/pisper/stargazers"><img src="https://img.shields.io/github/stars/ling-kong-ran/pisper?style=flat-square&color=8B5CF6" alt="GitHub Stars" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-8B5CF6?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="https://github.com/ling-kong-ran/pisper/releases/latest"><b>下载桌面版</b></a> ·
  <a href="https://pisper.cc/guide.html">使用教程</a> ·
  <a href="https://pisper.cc">官网</a> ·
  <a href="https://pisper.cc/changelog.html">开发日志</a> ·
  <a href="./README.en.md">English</a>
</p>

<p align="center">
  <img src=".github/readme/hero.webp" width="880" alt="Pisper 桌面端会话：代理重构完登录模块并跑完测试；右下角是手机上的“全部代理”，等你批准的命令可以直接允许或拒绝" />
</p>

## Pisper 是什么

Pisper 把多个 AI 编码代理组织成一个长期运行的工作台。每个代理有自己的会话、目录、模型和权限；侧栏按“需要你、运行中、最近”分组，谁在等你批准一眼可见，高风险命令就地允许或拒绝。

桌面（Tauri）、终端（Ratatui）与手机（Android / iOS）运行同一套 Node Runtime，会话、记忆和配置只保存在你自己的设备上。底层 Agent Runtime 是 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent)。

## 功能

- **并行代理**：多个会话同时推进，批准和提问以卡片形式出现在对话里；`Ctrl/⌘ + K` 跨会话直达。
- **会话树**：从任意回合另开分支、按轮回退文件、给回合打标记，原来的对话一字不改。
- **审批与权限规则**：审批后写入、自动审批、完全访问三档；按命令前缀批量允许或禁止，危险命令单独拦下。
- **子代理与推进方式**：fast / standard / strong 三档各配一个模型；Plan、Goal、Team 三种方式推进任务。
- **浏览器、电脑与远程机器**：代理在右侧浏览器里操作网站并沿用你的登录；Computer Use 操作桌面应用；经 SSH 或执行器在另一台机器上运行。
- **工具、插件与 MCP**：核心工具常驻，插件、MCP 和技能按需激活；缺能力时代理自己写插件；可开启内置 MCP 服务供其他本机代理调用。
- **记忆、工作流与定时任务**：偏好和决策直接记住并可追溯；Prompt、文件、MCP 与通知连成工作流；到点自动开跑，结果经飞书、微信等渠道推送。
- **模型与备用模型**：多 Provider 配置，主模型不可用或额度用尽时按顺序切到备用模型接着跑。

另有语音输入与对话模式、原生组件、Git / SVN 工作区、桌面宠物等，详见[使用教程](https://pisper.cc/guide.html)。

<p align="center">
  <img src=".github/readme/surfaces.webp" width="880" alt="同一个内核的另外两端：Ratatui 终端界面，以及代理在手机本机运行的会话" />
</p>

## Pisper Bot（实验功能）

Bot 是一个一直开着的会话，加上它是谁、管什么的设定。点开就接着聊，前情它都记得；改文件、跑命令它自己来，碰到危险操作才找你。

- **在聊天软件里叫它**：飞书、微信里发一句，它就去做。
- **定时让它做事**：每天巡检依赖、整理日报，到点自己开跑。
- **去另一台电脑上干**：把活派到远程机器上。
- **拉几个 Bot 进群**：@ 谁就叫醒谁；不 @ 时由群里判断该谁答。

Bots 默认关闭，在 **设置 › 实验功能** 里打开；关掉后 Bot 和它们的会话都不再显示，记录保留。见 [Bot 教程](https://pisper.cc/guide.html#bots)。

<p align="center">
  <img src=".github/readme/bot.webp" width="880" alt="和 Bot 小橙的长期对话：修好 Safari 白屏后接着部署预览环境，它记得刚才修的是哪一版" />
</p>

## 实测数据

下图是作者自己的使用记录，经 magpie 网关统计：680 个请求、32 个会话，模型为 glm-5.3、deepseek-v4-pro 与 kimi-k3.1-pro。评分与标签按所选时间范围内的请求算出。

<p align="center">
  <a href="docs/shots/prompt-analysis.png"><img src="docs/shots/web/prompt-analysis.webp" width="408" alt="magpie 网关统计的 Pisper 提示词体检：评分 100，缓存、起步轻、增速、可靠性四项满分" /></a>
</p>

| 指标 | 数值 |
| --- | --- |
| 评分 | 100（缓存 35/35、起步轻 25/25、增速 20/20、可靠性 20/20） |
| 典型提示词 / 起步 / 最大 | 64K / 3.3K / 565K |
| 每次增长 | +1.3K |
| 缓存命中 | 91% |
| 窗口大小 | 1.05M |
| 提示词构成 | 系统提示词 2% · 文件 14% · 工具结果 67% · 会话内容 16% |

系统提示词只占 2%，起步 3.3K，意味着上下文几乎都留给了你的文件和工具结果。这来自 Runtime 的几项设计：工具冷热分层、discover / call 按需加载、稳定排序的工具定义，以及提示词形态诊断——它们让 Provider 的 prompt cache 尽可能命中。

## 安装

**桌面版**：从 [Releases](https://github.com/ling-kong-ran/pisper/releases/latest) 下载，自带终端界面与 Runtime，不用装 Node.js。

- **Windows x64**：`*-setup.exe`；缺少 WebView2 的内网电脑用 `*-offline-setup.exe`。
- **macOS**：`*.dmg`，仅支持 Apple Silicon。
- **Linux x64**：`*.AppImage` 或 `*.deb`。

**手机 App**：安装包在 [App Releases](https://github.com/ling-kong-ran/pisper/releases?q=app-v)，Android 为已签名的 arm64 APK，iOS 为未签名 IPA（需要自己重签）。首次启动会直接进入内置的本机 Runtime——代理、工具和会话都在手机上运行，也能配对桌面。

**npm**：需要 Node.js 20 或更高版本。

```bash
npm i -g pisper
pisper            # 终端界面；首次用 /provider 配置模型
pisper web        # 打开网页界面
```

对话需要一个能访问到的模型服务（内网服务也可以）。macOS 提示“无法打开”、AppImage 起不来、手机怎么连桌面，见教程的[安装](https://pisper.cc/guide.html#install)、[手机](https://pisper.cc/guide.html#mobile)和[常见问题](https://pisper.cc/guide.html#faq)；终端命令见 [TUI 命令参考](./src-tui/README.md)；仍解决不了，去[支持](https://pisper.cc/support.html)页找我们。

## 数据安全

Pisper 没有“我们的云”。会话、记忆和配置都在你自己的设备上；只有你配置并实际调用的模型服务、外部 MCP、搜索和渠道，才拿得到对应任务需要的数据。

- **默认只听本机**：桌面 Runtime 绑定 127.0.0.1，手机本机 Runtime 只听随机回环端口；开启远程访问后才启动局域网 HTTPS 与 Iroh P2P，由 TLS 指纹和设备令牌保护。Pi 遥测默认关闭。
- **敏感先脱敏**：常见 API Key、Bearer / JWT、私钥与连接串在记忆落盘和摘要展示前被替换；凭据不经普通接口回显给代理，宿主 Shell 会移除常见凭据环境变量。
- **对外 MCP 自己开**：内置 MCP 服务默认关闭，只听 127.0.0.1，使用单独令牌。
- **记忆可追溯**：每次改动都有记录、可以撤销；你亲口说过的不会被模型的推断顶掉。

脱敏只识别常见格式，不是完整的 DLP、沙箱或端到端加密。Provider 凭据保存在本机数据目录（手机上在 App 私有目录，尚未迁入 Keystore / Keychain），请保护好这个目录和它的备份。详见[隐私说明](https://pisper.cc/privacy.html)。

## 开发

- **Runtime**：Node.js，ESM。HTTP / SSE 接口、领域服务、Pi 引擎接线与应用级工具都在这里；桌面端以 Node SEA 形式随应用打包。
- **界面**：React + TypeScript 的 Web 界面，Tauri 2 桌面与移动壳，Rust 编写的 Ratatui 终端界面。
- **独立更新**：Desktop、TUI、Runtime 与手机 App 各自有版本、签名和更新，更新失败自动回退到内置版本。

从源码运行需要 Node.js 24 与 npm：

```bash
git clone https://github.com/ling-kong-ran/pisper.git
cd pisper
npm ci
npm run dev     # Web 与 API，默认 http://127.0.0.1:5173
npm run check   # 类型、lint、i18n、格式与启动检查
npm test        # runtime/tests 下的全部测试
```

数据默认保存在 `~/.pisper/agent`，可以用环境变量 `PISPER_AGENT_DIR` 改到别处。仓库约定见 [AGENTS.md](./AGENTS.md)。欢迎提交 [Issue](https://github.com/ling-kong-ran/pisper/issues) 与 [Pull Request](https://github.com/ling-kong-ran/pisper/pulls)；请勿提交 API Key、机器人凭据或 `~/.pisper/agent` 中的个人数据。

## 致谢与许可

感谢 [Pi Coding Agent](https://github.com/earendil-works/pi/tree/main/packages/coding-agent)、[Petdex](https://petdex.dev) 及本项目使用的开源软件。贡献者：[@mik-myp](https://github.com/mik-myp)（前端架构、shadcn/ui 与 i18n 重构，[#1](https://github.com/ling-kong-ran/pisper/pull/1)）。

[MIT License](./LICENSE) · © Pisper Contributors
