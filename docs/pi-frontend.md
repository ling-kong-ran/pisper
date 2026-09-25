# ZCode / PI 前端分支

本分支为 `feat/zcode-visual-refresh`，后端基于远端 `release` 提交 `15f41936133cd7fdecca8a4c491b1127fe3f223a`（2026-09-24，Runtime 0.5.57）。它直接使用独立的 PI 简约界面，**不是** release 的模板导入包，也没有给 release 添加第二种界面。

## 开发与运行

使用 Node.js **24**，依赖保持原 `package-lock.json` 锁定版本。

```sh
npm ci
npm run dev
# 或生产模式：
npm run build
npm start
```

运行方式、Tauri 构建和各平台要求继续遵循 [开发流程](development-workflow.md)。默认后端数据目录仍是 `~/.pisper/agent`；不要与另一个 Pisper 实例同时打开同一数据目录。开发验证应通过 `PISPER_AGENT_DIR` 指定隔离目录。

## 与 release 的界面差异

- ZCode 风格的低对比度浅色/深色表面、圆角输入框和原创 PI 线框水印；没有复制 ZCode 标志、素材或程序代码。
- 默认常驻 **审批权限、Plan/Goal/Team、模型与智力**。模型与思考等级在同一个弹层中，沿用真实会话 API、禁用态与 Provider 能力检测。无推理能力或固定等级模型不会展示可用的假选项。
- 权限仍由后端会话状态决定，包括审批后写入、自动审批和完全访问。不会默认开启完全访问，也不会因为选择 Plan 自动修改权限。Plan 沿用 Pisper 的单轮语义，不代表强制只读。
- `+` 保留附件、Prompt/Skill/工具/工作流资源、图片生成、命令、手动压缩和会话操作。可固定、收纳、排序和恢复默认。快捷栏偏好使用 `pisper-zcode-composer-toolbar`，不覆盖 release 原来的偏好；旧的独立思考按钮由模型面板取代。
- 移除上下左右分屏与窗格关闭入口，只挂载活动会话。会话切换、历史、会话树、后台流式执行、草稿、队列与撤回仍使用 release 原逻辑；关闭视图不等于停止后端任务。
- 右侧上下文保留文件改动、计划与网页预览、键盘切换、调整宽度和窄屏抽屉。语音、审批、Token/缓存/上下文统计继续接原状态。
- 默认内容宽度 768、侧栏宽度 264、上下文初始关闭；完成时自动打开仍可配置。浮动组件按需添加，原有画布、组件库、布局 JSON、自定义样式与用户自定义布局仍可用；没有新增整套前端导入协议。
- 资产、工作流、计划任务、Provider、插件、MCP、技能、决策、记忆、通道、通知、快捷键、外观、远程连接、桌宠与更新路由均保留，系统能力仍以真实后端/宿主报告为准。

## 验证

```sh
npm run check
npm test
npm run build
node scripts/smoke-zcode-ui.mjs
```

UI 冒烟测试启动随机回环端口和临时后端数据目录，用本地 OpenAI-compatible SSE fixture 而非真实付费模型；Provider 网络发现和 GitHub 更新检查使用固定响应，真实更新请求另行验收。Windows 默认使用已安装的 Edge，其他环境可用 `PISPER_UI_BROWSER_PATH` 指定浏览器。测试结束后关闭服务并删除临时会话/配置，仅保留系统临时目录中的报告和截图供检查，可手动删除整个报告目录。不要将测试产物提交到仓库。

完整验证结果与未验证边界见 [验收记录](pi-frontend-validation.md)。

## 后续同步与回滚

只推送 `feat/zcode-visual-refresh`；`release` 不包含这些前端改动。以后在此分支合并新的 `origin/release`，重点检查会话 props、能力接口、布局默认值和快捷栏变更，再运行上述验证。回滚使用独立的 revert 提交，不强推、不改写远端历史。部署 release 的正常构建即可恢复其界面，后端数据格式没有变化。
