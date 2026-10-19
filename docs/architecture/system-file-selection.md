# 系统文件选择与工作目录边界

- 状态：已采纳
- 日期：2026-10-16
- 影响范围：React Web、Runtime、桌面端、Android、iOS；TUI 不消费被移除的目录枚举 API。

## 决策

删除自建的目录树和文件列表。桌面工作目录、附件分别复用 Tauri 的 `pickDirectory`、
`pickFiles` 系统对话框；本地文件链接继续通过现有 reveal 服务定位到系统文件管理器。
Web 和移动端附件统一使用浏览器的系统文件选择器上传内容，不再枚举 Runtime 宿主文件。

浏览器无法提供 Runtime 设备上的绝对路径，工作目录改为手动输入；
`WorkspacePicker` 仅持有路径草稿、提交和错误状态，不再请求目录列表。
Android 本机保留已有的系统 SAF 目录导入，返回可读写副本的 Runtime 路径；
远程模式不接受手机目录。iOS 保留路径输入与系统附件选择，不宣称支持 Android 的目录导入。
移动上下文页删除文件浏览标签，资产、改动审批/撤销、终端和文件预览保持原有职责。

## 删除与兼容性

- 删除 `PathAttachmentPicker.tsx`、`features/chat/api/workspace-entries.ts` 及专属文案。
- 删除 `/api/directories`、`/api/workspace-entries`、Runtime 对应方法与目录枚举实现。
- `workspace-directories.mjs` 继续持有路径归一化和目录存在性校验，供会话等领域用例调用。
- 不引入新状态存储或持久化迁移。移动文件页签原本仅为组件临时状态。

本次明确移除文件浏览接口；旧 Web 客户端应与 Runtime 一并更新，旧接口返回标准 404。
TUI 未引用这些端点，其 JSON/SSE 契约保持原状。不保留兼容枚举实现，以免继续暴露自建浏览接口。

## 验证与回滚

`runtime/tests/workspace-directories.test.mjs` 将原目录枚举断言替换为路由不可枚举及目录校验测试，
保留系统原生命令、权限和调用方的接线检查。`npm run test:mobile:ui` 使用真实组件验证
手动路径提交/错误、Android 本机导入/取消、远程模式隔离、Web/移动系统附件选择、
桌面附件路径/取消/失败，以及移动壳既有布局和生命周期行为。
浏览器测试替代系统对话框边界，不代表原生窗口或 Android/iOS 真机验收。

同时运行 `npm run check`、`npm test`、`npm run build`、TUI 检查与测试。
回滚应一起恢复前端入口、Runtime 路由/方法、文案和测试；不能覆盖独立保存在 stash 中的用户改动。

## 恢复工作台暂存改动时的整合边界

2026-10-16 恢复发版前暂存的工作台改造，保留任务、分屏、来源、进程和页面模板等原改动。
删除其中新增的 `TaskFilesPane`、`workspace-entries*.ts` 以及 `workspace-files` 资源标签、菜单和视图状态，
保持本记录中的文件浏览退役决定。项目列表的系统目录选择接线随组件重命名迁移到
`features/chat/ProjectTaskList.tsx`；验收夹具使用同一生产组件。已发布的各渠道版本元数据保持当前值。
恢复使用 `git stash apply`，原 stash 留作备份；本次恢复不自动提交或再次发布。
