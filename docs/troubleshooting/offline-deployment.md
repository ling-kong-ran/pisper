# 离线安装与内网使用审计

本页记录 2026-09-22 对安装、首次启动、会话打开、本地工具及资源交付路径的检查。
修复的源码与构建验证记录如下；正式交付版本以包含这些变更的发布标签和产物为准。

“离线可用”指安装和本地基础功能不需要公网；模型对话仍需要可达的模型服务，可以是内网端点。
这不等于应用完全不发起外网请求：更新检查、公共模型目录和赞助内容仍可能尝试联网，但不应阻断会话。

## 安装包选择

- Windows：优先选择普通 `*-setup.exe`（不含 `-offline-`）。已有 WebView2 时可断网安装；缺失时需要联网补齐。无 WebView2 或无法确认的离线电脑，选择 `*-offline-setup.exe`，它额外携带完整 WebView2 安装器。自动更新使用普通包。
- macOS：按 Apple Silicon / Intel 选择 DMG，使用系统 WKWebView，无需额外内核包。
- Linux：AppImage 可能需要 FUSE；DEB 适用于 Debian / Ubuntu，可能需要 WebKitGTK 等发行版依赖。断网前准备系统库，不保证所有 Linux 系统开箱即用。
- Android：使用已签名 APK 和系统 WebView，首次侧载按系统提示允许安装来源。
- iOS：使用系统 WKWebView，公开 IPA 未签名，必须自行重签后安装。

所有桌面包包含 TUI 与 Runtime；完整离线包不包含按需下载的本地语音模型。各平台的模型、扩展和外部程序仍需单独准备。下载页以 Release 实际资产为准，旧版本没有独立离线包时会回退到发布页。

## 发现与修复

### 缺少依赖时会显示什么

- Windows 普通安装包：检测到 WebView2 缺失时，在开始安装依赖前显示中英文说明，明确它用于界面显示、需要联网下载，并允许取消改用完整离线包。失败时保留安装错误码并给出离线包地址、权限和系统兼容性排查方向。无人值守安装不弹出确认框，保留详情输出。
- Windows 完整离线包：检测到 WebView2 缺失时说明将使用内置安装器，不下载语音模型。
- Windows 已安装应用：WebView2 被卸载或无法加载时，显示系统原生提示，不依赖无法创建的网页窗口。
- 桌面 Runtime / 前端文件：已知随包入口缺失会指出相对文件名；Runtime 启动或原生模块加载失败、桌面初始化失败分别显示错误码及修复方向，不把未知失败归因为 WebView2，也不展示个人路径或凭据。
- Linux DEB：用 `sudo apt install ./Pisper_*_linux_x86_64.deb` 安装，包管理器会列出未满足依赖。断网前准备对应发行版的依赖包。AppImage 若提示 `libfuse.so.2`，可安装发行版的 FUSE 2 兼容包，或使用 `./Pisper_版本_linux_x86_64.AppImage --appimage-extract-and-run` 解包运行。若出现 `GLIBC_* not found`、缺少 `.so` 或图形驱动错误，记录完整库名与系统版本反馈；不要把它当成 WebView2 缺失。

系统动态加载器在 Pisper 执行前就失败时，应用无法弹窗，需从终端启动读取系统错误。此类错误与应用启动后可捕获的失败分开处理，不能承诺所有缺失系统库都由 Pisper 弹窗诊断。

| 路径 | 风险 | 处理 |
| --- | --- | --- |
| Windows 安装器 | 缺少 WebView2 时，在线 bootstrapper 在内网无法补齐依赖 | 普通包复用已有 WebView2，缺失时联网安装；完整离线包额外携带 WebView2。0.5.72/0.5.73 的原安装包也包含完整离线安装器 |
| Pi `grep` / `find` | 干净系统没有 `rg` / `fd` 时，首次调用隐式访问 GitHub 下载工具 | Runtime 构建阶段按平台携带固定版本、SHA-256 和许可证；运行时优先使用随包工具 |
| 会话资源发现 | 配置中存在尚未安装的 npm/git 扩展时，打开会话会自动安装，受 npm/GitHub 可达性影响 | 发现阶段跳过缺失包；保留配置及管理界面的未安装状态，安装/更新由用户显式触发 |
| 未知模型元数据 | 打开会话和切换内网模型等待 `models.dev`；失败后每次重复等待 | 使用已有模型配置，元数据后台补充；网络失败冷却五分钟，退出时取消并等待查询结束 |
| Web 更新检查 | 未设置请求超时，公网不可达时检查可能长期挂起 | 请求增加十秒超时；不影响已加载应用 |

## 已核对的交付与生命周期

- 桌面/TUI SEA 分发包携带 Node 和生产 JS 闭包。npm 安装器读取对应 npm 平台包中的签名归档，不在 postinstall 另行从 GitHub 拉取 Runtime；离线 npm 安装仍需预先镜像/缓存 npm 包及匹配的平台可选依赖，宿主 Node 也必须满足开发流程文档中的要求。
- 字体来自本地 `@fontsource` 资源，前端启动资源由 Vite 打包。本次扫描未发现首屏必须依赖外部字体/CDN 的路径。用户消息中的远程图片、链接和自定义扩展的网络访问不属于内置资源保障。
- 桌面 OCR 的英文、简体中文模型以及 Tesseract LSTM WASM 在构建时校验并打入闭包，不靠首次识别下载语言包。
- 浏览器自动化使用 `playwright-core` 和系统已有的 Edge/Chrome/Brave；没有安装浏览器时，这项功能仍不可用。Pisper 不在首次启动下载浏览器，WebView2 也不能替代完整浏览器。
- Windows 命令工具可以使用系统 PowerShell；基础使用不要求用户安装 Bash。模型自行执行的 Python、第三方命令和项目依赖不在应用安装包的通用承诺内。
- MCP 初始化读取配置，不在启动时自动连接。已启用的消息通道会在后台重连，公共模型目录也可能后台刷新；与会话主路径分离。
- 语音模型、联网搜索、扩展市场、MCP 外部程序、远程宠物资源及云模型是独立前提：内网部署须事先准备对应资源或服务。不能据此宣称“全功能完全离线”。

## 搜索资源与构建维护

资源清单位于 `scripts/search-tool-resources.json`，包含上游地址、版本、归档 SHA-256、归档目录与许可证。
当前固定 ripgrep 15.2.0（MIT / Unlicense）和 fd 10.5.0（MIT / Apache-2.0），来自各自上游 GitHub Release。
构建下载缓存位于被 Git 忽略的 `release/cache/search-tools/`；每次使用均校验摘要。
只把目标平台程序、许可证和来源记录放入
`node_modules/@earendil-works/pi-coding-agent/vendor/bin/`，由关键文件清单及体积预算检查。
升级时需同步版本、URL、摘要和许可证，并在所有交付平台运行 SEA 冒烟测试；不得改为运行时拉取 latest。

`scripts/patch-pi-offline-compat.mjs` 通过现有 Pi postinstall/staging 补丁入口接入。
补丁在上游源码不匹配时使构建失败，不直接维护供应商文件。
它只调整工具路径优先级和资源发现的默认安装策略，保留显式安装接口。
源码开发环境未执行资源 staging 时仍可使用宿主工具或 Pi 的原下载机制；离线交付保证针对已完成 staging 的分发包。

这些修复不迁移用户配置、HTTP/SSE 协议或存储格式。构建脚本拥有搜索资源清单，Pi 适配层消费随包文件；
模型元数据服务拥有后台请求、冷却和停止状态，由 Runtime 负责关闭。
回滚应整体恢复补丁、staging 和对应测试，不能仅撤下二进制却保留离线交付声明。

## 平台边界与验收

- Windows x64：已检查本次选定 `rg.exe` / `fd.exe` 的 PE 导入，仅包含 Windows 系统 DLL，未引入额外 VC++ Redistributable 依赖。静态检查不能替代干净 Windows 虚拟机的实际运行。
- macOS：本次 ARM64 二进制声明最低 macOS 11；不引入额外包管理器。实际验收结果见本次变更记录。
- Linux：使用 musl 搜索程序；桌面壳本身仍需发行版提供 WebKitGTK 等系统库，不能把一个 `.deb` 当作所有 Linux 系统依赖的离线集合。AppImage 的 FUSE/系统兼容性也需按目标发行版验证。
- Android / iOS：共享元数据与扩展发现修复适用；桌面搜索二进制不会进入移动闭包。移动端进程能力沿用现有 capability；本次没有增加 Android `rg`/`fd` 交付或 iOS 子进程支持，不宣称移动端新增了离线搜索能力。

自动验证入口：

```bash
npx tsx --test runtime/tests/offline-runtime.test.mjs runtime/tests/model-metadata.test.mjs runtime/tests/search-tool-staging.test.mjs
npm run check
npm test
npm run sidecar:sea
npm run sidecar:sea:smoke
```

新增 SEA 冒烟测试启动独立进程，清空 PATH、隔离用户目录并拒绝 fetch，实际运行 `grep` / `find`，
覆盖中文和空格路径及 `.gitignore`。这能防止构建机预装工具或个人缓存掩盖漏包。
仍需使用完整离线包在无 WebView2、无 Node/Python/Bash/rg/fd、断开公网的 Windows 环境完成安装、重启和内网模型对话验收；普通包需分别验证已有 WebView2 的离线安装和缺失 WebView2 的联网安装。
同时验证升级安装和已有数据目录。没有这项真机结果时，不得声称该场景已经验收通过。

本次本地验证：macOS ARM64 的质量检查、完整 Node 测试、SEA 构建及冒烟通过；
新增隔离测试验证了拒绝公网 fetch 时首次启动和重启后打开内网模型会话，元数据仍在等待也不会阻塞。
Windows x64 两份固定摘要的归档在本机完成校验、解包和交付文件检查，但未执行 Windows 程序。
新闭包为 184.9 MiB，较补齐搜索工具前增加约 7 MiB，仍通过原有体积预算。
Windows、Linux、Intel macOS 的实际执行以及移动端构建/设备运行尚未验证。
