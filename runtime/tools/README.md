# Application tools

应用级 Agent 工具统一放在 `app/`，一个模块一组工具（如 `memory.mjs` 含搜索与记忆两件）。

进入插件目录的工具模块须导出：

- `manifest(s)`：插件页面使用的名称、分类、风险和能力说明。
- `create…Tool(context)`：返回 `defineTool()` 创建的工具定义。

工具工厂通过参数接收 `cwd` 或后续服务依赖，不应直接引用 AgentRuntimeService。
goal / plan / 多 Agent / 工具发现与网关是内部运行时工具，刻意不进插件目录。
注册集中在 `app/index.mjs`；目录合并、权限校验与预设集中在 `registry.mjs`
（配合 `builtin-catalog.mjs`）与 `ToolPluginService`。

## codemode

`codemode` 是 Pi 自带的脚本编排工具，由 `createCodemodeExtension()` 按能力（`features.codemode`，仅桌面）注册，
默认不在预设里，需在插件页开启。脚本里调用的工具不经过 `agent.beforeToolCall`，
`runtime/runtime/nested-tool-calls.mjs` 把它们接回同一条钩子链，审批、文件快照与 Team 文件范围照常生效；
新增的 `beforeToolCall` 钩子仍挂在 `agent.beforeToolCall` 上即可。Pi 的内置 `mcp` / `tool-search` 扩展刻意不装。
