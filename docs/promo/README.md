# Pisper 宣传素材

本目录交付「一次灵感，多路推进」60 秒中文宣传片，项目截图同步更新至 Runtime/Web 0.5.37、TUI 0.5.23 对应界面。

## 成片与使用

- [宣传片 MP4](pisper-promo-zh-CN.mp4)：1920 × 1080，30 fps，H.264 / AAC，60 秒；包含中文画面标题、配乐与语音输入示范音频。
- [PNG 封面](pisper-promo-cover.png) / [WebP 封面](pisper-promo-cover.webp)。
- [中文字幕 VTT](pisper-promo-zh-CN.vtt) / [英文字幕 VTT](pisper-promo-en-US.vtt) / [中文字幕 SRT](pisper-promo-zh-CN.srt)。
- 项目主页 `../index.html#promo` 提供带播放控制与字幕的播放器，默认不自动播放、不预加载视频。
- [创意与分镜](creative-brief.md)、[拍摄清单与校验](capture-manifest.json)。

## 最终时间轴

| 时间 | 内容 |
| --- | --- |
| 00:00–00:06 | 想法，不必排队：品牌与工作空间 |
| 00:06–00:12 | 语音对话模式的实时点云与聆听状态 |
| 00:12–00:19 | 点击语音输入，识别“帮我梳理实现思路”，文字进入草稿 |
| 00:19–00:26 | SSE 回复逐步渲染与跟随滚动，连续原速镜头 |
| 00:26–00:33 | 真实双会话分屏 |
| 00:33–00:38 | 会话树与分支探索 |
| 00:38–00:44 | 可视化工作流与审批节点 |
| 00:44–00:50 | 技能、MCP 配置与记忆星图 |
| 00:50–00:54 | 桌面、TUI、移动布局组合 |
| 00:54–01:00 | 品牌与官网地址 https://pisper.cc |

最终剪辑以此时间轴为准；创意简报保留前期策划供后续改版参考。

## 拍摄来源与边界

全部内容使用独立演示目录 `E:/pisper-media-tools/demo-data` 与公开编写的示例任务，不使用私人会话、Provider 密钥或个人工作文件。修改对象为宣传素材及文档，没有修改产品源码。

- **桌面/Web 界面**：当前源码启动独立 Runtime，在 Edge 中拍摄真实 React 页面；会话、工作流、资产、记忆、技能、MCP 配置与定时任务均为演示数据。未执行定时任务，MCP 示例明确保持禁用。文件原图为 1600 × 1000。
- **语音输入**：使用项目现有目录中的 X-ASR 480 ms INT8 模型副本实际识别。示范句由 MeloTTS 本地模型生成，通过 Web Audio 注入浏览器麦克风输入，随后经过应用真实采集、识别与草稿更新逻辑。没有直接写入伪造的识别结果。影片中的示范人声为相同音频。
- **对话模式**：拍摄真实点云动画与聆听状态。此镜头用于展示对话入口和界面，不作为整轮语音对话成功率或端到端延迟测量。
- **流式回复**：本地 OpenAI 兼容演示服务提供预先编写的文本，经 Runtime `/api/chat` SSE 和现有前端渲染、滚动逻辑显示。画面使用连续 7 秒原速录屏，未用后期打字或位移动画伪造滚动。内容提供方是演示服务，不是外部模型推理；此片不宣称模型速度、设备性能或所有场景下的帧率保证。
- **TUI**：当前源码一次增量构建得到 `pisper 0.5.23`；Windows ConPTY 捕获实际 ANSI 输出，经 xterm.js/Edge 截图。原图 1587 × 830，PNG 内嵌版本、命令、ANSI 与摘要信息。
- **桌面更新页**：当前 React 更新设置页，拍摄桥接信息使用仓库 Desktop manifest 的 0.5.55 版本，保持“尚未检查”；未以演示素材宣称检查远程更新或安装成功。
- **终端面板**：当前 React TerminalPanel/xterm，使用拍摄用桌面桥接适配器展示在演示目录实际执行的 PowerShell / `node --version` 输出。没有宣称这次拍摄验证了 Tauri 原生终端传输。
- **移动端**：当前 React 移动布局在 430 × 932 viewport、DPR 2 的 Edge 中拍摄，输出 860 × 1864。服务器页由移动壳状态 fixture 提供公开示例数据。这两张图不是 Android/iOS 真机截图，也不代表验证了原生配对、本机 Runtime 启动或设备生命周期。

旧的 27 张 PNG 与对应 WebP 均已更新，新增 `conversation-mode`、`voice-input`、`smooth-streaming` 三组，共 30 组。原图入口保持原文件名，README、主页及界面展示页继续使用对应路径。

## 制作与复用

所有制作工具、依赖、模型副本、录屏中间文件与可编辑工程均位于项目外：

```text
E:/pisper-media-tools/
  film/index.tsx           # Remotion 时间轴、镜头、中文标题
  film/public/            # 音乐、源录屏及素材引用
  production/music.py     # 本次原创合成电子配乐
  production/record.mjs   # CDP 录屏与时间戳保存
  production/provider.mjs # 本地演示内容服务
  production/render.mjs   # 正式导出命令
  production/takes/       # 原始录屏帧及采集证据
  venv/                   # Python 工具环境
  tui/                    # TUI 捕获与构建日志
```

复用已有 `E:/remotion` 中的 Remotion 4.0.508 与 Chrome Headless Shell，以及系统 FFmpeg；未将工具依赖安装进本仓库。导出时工作目录为 `E:/pisper-media-tools/film`，避免将制作缓存写进项目。

```bash
node E:/pisper-media-tools/production/render.mjs
```

该命令依赖本机上述外部工程与工具目录，并非仅凭仓库即可重拍的端到端脚本。需要迁移制作环境时，应一并复制外部 `film` 与所用原始录屏，重新配置工具路径。

视觉使用本项目品牌素材；背景光线、镜头包装与电子配乐为本次程序生成，没有使用下载的商业音乐、库存视频或生成式视频片段。中文画面使用 Windows Microsoft YaHei 系统字体，字体文件不随仓库分发。本地 TTS 模型为项目 catalog 中的 MeloTTS Chinese + English（MIT）；ASR 为 X-ASR（Apache-2.0），模型本体不随宣传素材分发。使用 Remotion 制作的后续商业项目应遵循其适用许可条款。
