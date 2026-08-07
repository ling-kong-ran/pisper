# Application tools

应用级 Agent 工具统一放在 `app/`，一个工具一个模块。

每个模块必须导出：

- `manifest`：插件页面使用的名称、分类、风险和能力说明。
- `create...Tool(context)`：返回 `defineTool()` 创建的工具定义。

工具工厂通过参数接收 `cwd` 或后续服务依赖，不应直接引用 AgentRuntimeService。注册步骤集中在 `app/index.mjs`；权限校验和预设集中在 `registry.mjs` 与 `ToolPluginService`。
