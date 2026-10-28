# 前端还原阶段性提交（2026-10-28）

本次按用户要求创建分支并提交、推送现有工作，保存设计分析、差异清单、前端实现与验证资产。当前为 WIP，尚未完成全部页面还原或最终验收。其他文档中的历史验证结果仅对应各自记录的检查点，不代表本次完整工作区已通过。

参考设备上已记录的 ChatGPT 版本为 26.1013.40934（build 20770）；Pisper 的名称、Logo、欢迎语、模型名称和功能文案继续使用自身内容。

## 本次验证

- `npm run typecheck`：通过。
- `npm run check`：类型、Lint（有警告）、i18n 检查通过；格式检查未通过。本次涉及的 12 个文件随后已格式化，重新执行 `npm run format:check` 仅剩忽略目录 `.superpowers/` 中另一任务的两个 Markdown 文件失败，未修改或纳入提交。
- `npm run test:startup`：独立执行通过，1 项测试。
- `npm test`：2781 项，2774 项通过、5 项失败、2 项跳过。
- `npm run tui:check` 与 `npm run tui:test`：通过。
- `git diff --cached --check`：提交前检查；未发现空白错误。

全量测试失败项保留，尚未作为本次保存工作的前置修复范围：

1. `asset UI previews text and video while preserving a web download fallback`
2. `asset page exposes a dedicated video filter`
3. `asset cards grow with their copy instead of clipping the action row`
4. `locale resources use single-brace interpolation placeholders`
5. `modal surfaces stay scrollable within low-height viewports`

## 尚待完成

通知设置、资产页面、未保存变更导航保护及字号设置仍需继续集成和验收。此前完整 UI 测试记录过模型菜单 Escape 关闭和流式最终文本未完成的问题，尚未完成闭环；本次未重新执行完整 UI、生产构建及桌面和移动端设备验收，也未进行与参考应用的像素对照验收。

这次提交不包含版本发布、发布产物或真实用户代理数据。回退实现应使用可追踪的回滚提交，保留分析文档作为后续修复依据。
