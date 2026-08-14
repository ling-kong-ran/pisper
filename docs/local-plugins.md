# 本地插件

Pisper 本地插件是一个包含 `pisper-plugin.json` 和 JavaScript 入口文件的目录。插件可以提供一个或多个 Agent 工具；安装、启停和卸载以插件为单位管理，单个工具可独立启停。

当前版本只支持本地目录安装，不支持插件市场、npm/Git 下载、自动更新、原生模块、生命周期脚本、Provider 注入、TUI UI 或覆盖内置工具。

## 清单

在插件目录根部创建 `pisper-plugin.json`：

```json
{
  "schemaVersion": 1,
  "id": "example.echo",
  "name": "Echo Plugin",
  "version": "1.0.0",
  "description": "Example local plugin.",
  "entry": "index.mjs",
  "permissions": ["workspace-read"],
  "tools": [
    {
      "name": "example_echo",
      "label": "Echo",
      "description": "Return the supplied text.",
      "scope": "Current chat workspace",
      "parameters": {
        "type": "object",
        "properties": {
          "text": { "type": "string" }
        },
        "required": ["text"]
      }
    }
  ]
}
```

约束：

- `schemaVersion` 当前只能为 `1`。
- `id` 使用小写字母、数字、点和连字符，长度为 1-96。
- `version` 使用语义版本格式，例如 `1.0.0`。
- `entry` 必须是插件目录内的 `.js`、`.mjs` 或 `.cjs` 文件。
- 每个插件声明 1-32 个工具；工具名使用小写字母、数字和下划线，且不能与内置或其他已安装工具重名。
- `parameters` 是 JSON Schema 对象，供 Agent 调用参数校验使用。
- 清单不能超过 256 KB；整个目录最多 512 个文件和 20 MB，且不能包含符号链接。

## 入口

入口模块导出 `execute` 函数。Pisper 每次调用都会启动一个独立 Worker：

```js
export async function execute({ toolName, arguments: input, context }) {
  return {
    content: [
      {
        type: 'text',
        text: `${toolName}: ${input.text}`,
      },
    ],
    details: {
      sessionId: context.sessionId,
    },
  }
}
```

`context` 包含：

- `cwd`：当前会话工作目录。
- `sessionId`：当前会话 ID。
- `dataDir`：插件持久化数据目录。

返回值可以是字符串，或包含 `content` 数组的 Pi 工具结果。单次结果上限为 1 MB，执行超过 120 秒会被终止，取消 Agent 调用也会终止 Worker。

## 让 Agent 创建插件

在“完全访问”执行模式下，内置 `plugin_create` 工具可以通过结构化参数一次定义插件清单、一个或多个工具、`index.mjs` 入口代码和附加文本文件。它会执行与“插件”页面相同的清单、Schema、路径、体积和工具名冲突检查，并在通过后直接安装。

生成的源码固定写入 Pisper 全局 Agent 目录下的 `plugin-sources/<plugin-id>`，不归属于某个项目。工具不会覆盖已有源码目录或已安装插件；创建或安装失败时，只会清理内容仍与本次生成结果一致的文件。安装成功后，新工具对所有项目可用，并从下一次 Agent 请求开始通过 `discover_tools` 和 `call_tool` 调用。插件执行时的 `context.cwd` 仍指向当前会话工作目录。

用户可以直接要求 Pisper 创建所需能力，例如“创建一个读取项目版本号的本地插件并安装”。Agent 应使用 `plugin_create`，而不是手工拼接清单后绕过统一预检。

## 安装与安全

在 Pisper 的“插件”页面选择“安装插件”，输入或选择插件目录，先执行静态检查，再确认安装。检查完成后如果来源目录发生变化，安装会被拒绝并要求重新检查。

插件代码虽然在独立 Worker 中运行，但这不是操作系统沙箱。它仍可使用当前系统用户有权访问的文件和网络，也可以加载 Node.js 模块。因此第三方插件固定视为高风险，只会在会话的“完全访问”执行模式下提供给 Agent。只安装来源可信且已审阅的代码。

内置工具不可卸载。卸载本地插件会一次移除该插件和它提供的全部工具；插件执行期间不能卸载。
