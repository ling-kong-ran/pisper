# Pisper TUI 命令参考

[项目主页](https://ling-kong-ran.github.io/pisper/) · [GitHub 仓库](https://github.com/ling-kong-ran/pisper) · [English](./README.en.md)

参数约定：`<参数>` 必填，`[参数]` 可选。

## CLI 命令

| 命令 | 解释 |
| :--- | :--- |
| `pisper` | 在当前目录启动 TUI，并创建一个空会话；不会自动恢复历史会话。 |
| `pisper --cwd <目录>` | 使用指定目录启动 TUI 并创建空会话。 |
| `pisper resume` | 打开跨工作目录的历史会话选择器；没有历史会话时提示后退出。 |
| `pisper resume --cwd <目录>` | 从指定启动目录进入历史会话选择器；恢复后仍使用会话自身保存的工作目录。 |
| `pisper doctor` | 检查 Sidecar、Runtime 鉴权、能力目录与最近会话目录，并输出诊断信息。 |
| `pisper doctor --cwd <目录>` | 以指定目录作为启动工作区执行诊断。 |
| `pisper web` | 安装或使用 Web 前端，并在默认浏览器打开本机认证配置页；命令运行期间需保持进程存活。 |
| `pisper web --cwd <目录>` | 以指定目录启动 Runtime 并打开 Web 配置页。 |
| `pisper help [COMMAND]` | 显示全局帮助；`pisper help web` 显示 Web 子命令帮助。 |
| `pisper -h` / `pisper --help` | 显示命令帮助，不启动 Runtime。 |
| `pisper -V` / `pisper --version` | 显示已安装的 TUI 版本。 |
| `pisper update --check` | 检查 npm 安装的 Pisper 是否有更新，不执行安装。 |
| `pisper update` | 使用当前 npm registry 更新 npm 安装的 Pisper，并重新校验组件。 |

## 环境变量

| 变量 | 解释 |
| :--- | :--- |
| `PISPER_TUI_MOUSE=1` | 启用鼠标滚轮滚动聊天记录；注意鼠标捕获会接管终端原生文本选择。 |

## Slash Command

| 命令 | 解释 |
| :--- | :--- |
| `/init` | 分析当前项目并创建或完善项目根目录的 `AGENTS.md`；保留已有有效说明，不修改其他项目文件。 |
| `/new` | 在 TUI 启动目录创建空会话；Agent 运行期间不可执行。 |
| `/sessions` | 打开跨所有目录的历史会话选择器；Agent 运行期间不可切换会话。 |
| `/dir <目录>` | 修改当前会话的工作目录；相对路径以当前会话目录为基准，Agent 运行期间不可修改。 |
| `/changes` | 打开 Git 或 SVN 改动视图。视图内 `R` 刷新、`C` 提交、`P` 推送 Git、连续两次 `V` 撤销；SVN 不提供 Push。 |
| `/changes commit <message>` | 使用指定提交信息提交当前 Git 或 SVN 工作区改动。 |
| `/chat` | 从其他视图返回 Chat 消息流。 |
| `/model` | 打开模型选择器并切换当前会话模型；只列出已配置 Provider 的模型，Agent 运行期间不可切换。 |
| `/thinking` | 刷新并选择当前模型支持的思考等级；Agent 运行期间不可切换。 |
| `/provider [id]` | 编辑 Provider 协议、当前生效 Base URL 与掩码 API Key；传入 `id` 可直达指定 Provider。 |
| `/apikey [id]` | `/provider` 的兼容别名。 |
| `/web` | 使用已安装的 Web 前端，在默认浏览器打开本机认证配置页。 |
| `/compact` | 立即摘要较早上下文；仅适用于有足够历史且当前空闲的会话。 |
| `/attach` | 打开当前会话目录的附件选择器。 |
| `/mode` | 显示当前执行模式及可用值；Agent 运行期间也可调整。 |
| `/mode approval-required` | 写入、Shell 和高风险工具执行前请求授权。 |
| `/mode workspace-write` | 自动批准工作区修改和常规命令。 |
| `/mode full-access` | 允许以当前系统用户权限访问本机文件、网络与 Shell。 |
| `/quit` | 退出 TUI。 |

## Tool 与 Skill 命令

| 形式 | 解释 |
| :--- | :--- |
| `/<tool> [请求]` | 请求 Runtime Tool，例如 `/read README.md`、`/bash npm test` 或 `/web_search Pisper`。Agent 仍负责生成参数并调用 Tool，当前执行模式与 Tool 开关继续生效。 |
| `/skill:<name> [请求]` | 请求已启用 Skill，例如 `/skill:docs-search 查找签名要求`。Runtime 按需加载 Skill 资源，其中触发的 Tool 继续经过同一权限链。 |
