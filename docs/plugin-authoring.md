# Pisper 插件开发指南（规范与格式）

本文档面向**插件开发者**，说明 Pisper 本地插件的目录结构、清单（manifest）规范、入口文件格式、工具定义 JSON Schema、运行时约束与安全边界。

面向使用者的安装与操作说明见[本地插件指南](./local-plugins.md)。

## 概念：插件与工具

Pisper 严格区分两个层级：

- **插件（Plugin）**：安装、启停和卸载的管理单元。一个插件是一个包含清单和入口代码的目录，可对外提供一个或多个工具。
- **工具（Tool）**：Agent 的调用协议。Agent 通过 `discover_tools` 发现、通过 `call_tool` 调用工具，工具名必须全局唯一。

插件通过清单声明工具，运行时把每个工具暴露为独立的 Agent 工具。工具本身不能由前端直接执行；可调用范围由 Runtime 权威返回。

## 目录结构

一个插件就是一个普通目录，最小结构如下：

```
my-plugin/
├── pisper-plugin.json   # 必需：插件清单
└── index.mjs            # 必需：入口文件（由清单 entry 指定）
```

目录内可以再放入口代码引用的其他相对文件（JS、JSON、文本等）。整个目录连同清单一起被复制到全局插件安装目录，安装后原目录不再被引用。

## 清单 `pisper-plugin.json`

清单是插件目录根部的 JSON 文件，声明插件身份与它提供的工具。

```json
{
  "schemaVersion": 1,
  "id": "example.project-package-info",
  "name": "Project Package Info",
  "version": "1.0.0",
  "description": "Read package metadata from the current chat workspace.",
  "entry": "index.mjs",
  "permissions": ["workspace-read"],
  "tools": [
    {
      "name": "project_package_info",
      "label": "Project package info",
      "description": "Read the current project's package.json and return its name, version, description, package manager, and npm script names.",
      "scope": "The package.json file at the root of the current chat workspace",
      "parameters": {
        "type": "object",
        "properties": {},
        "additionalProperties": false
      }
    }
  ]
}
```

### 顶层字段

| 字段 | 必填 | 类型 | 约束 |
| --- | --- | --- | --- |
| `schemaVersion` | 否 | number | 当前只能为 `1`，缺省视为 `1` |
| `id` | 是 | string | 1-96 位，仅小写字母、数字、点 `.` 和连字符 `-`，首尾必须是字母或数字 |
| `name` | 是 | string | 1-100 字符 |
| `version` | 是 | string | 语义版本，如 `1.0.0`；可选预发布后缀，如 `1.0.0-beta.1` |
| `description` | 否 | string | 最多 1000 字符（超出截断） |
| `entry` | 是 | string | 插件目录内的相对路径，仅 `.js` / `.mjs` / `.cjs` |
| `permissions` | 否 | string[] | 最多 32 项，去重后的**声明性权限说明**，安装前展示给人审阅 |
| `tools` | 是 | object[] | 1-32 个工具，见下 |

`id` 与 `version` 共同决定安装位置 `<pluginRoot>/<id>/<version>/`，因此同一 `id` 不同 `version` 可以并存（但当前版本不支持对已安装 `id` 的覆盖安装）。

`permissions` 是展示用元数据，不是强制访问控制；插件代码实际仍以当前系统用户身份运行，见[安全边界](#安全边界)。

### 工具对象 `tools[]`

| 字段 | 必填 | 类型 | 约束 |
| --- | --- | --- | --- |
| `name` | 是 | string | 1-64 位，以小写字母开头，仅小写字母、数字、下划线 |
| `label` | 否 | string | 最多 100 字符，缺省用 `name` |
| `description` | 是 | string | 最多 1000 字符；应写清工具做什么、何时调用 |
| `scope` | 否 | string | 最多 500 字符，说明可影响的文件/服务；缺省用 `description` |
| `parameters` | 否 | object | 描述入参的 JSON Schema，`type` 必须为 `object` |

约束：

- 工具 `name` 不能与内置工具（如 `plugin_create`、`skill_create`、`web_search` 等）重名，也不能与其他已安装插件的工具重名。
- 同一插件内的工具 `name` 不能重复。
- `parameters` 必须是有效的 JSON Schema；`type` 必须为 `object`，`properties`（若存在）必须是对象，`required`（若存在）必须是字符串数组。Runtime 用它校验 Agent 传入的参数。

## 入口文件

入口文件导出 `execute` 函数。每次工具调用，Runtime 都会启动一个独立 Worker 加载该文件：

```js
export async function execute({ toolName, arguments: input, context }) {
  return {
    content: [{ type: 'text', text: `called ${toolName}` }],
    details: {},
  }
}
```

### 导出解析

Runtime 按以下顺序解析入口：

1. `module.execute`
2. `module.default?.execute`
3. `module.default`

因此默认导出函数本身、默认导出 `{ execute }` 对象，或具名导出 `execute` 均可。

### 入参对象

`execute` 接收单个对象：

- `toolName`：本次调用的工具名（一个插件提供多个工具时据此分派）。
- `arguments`：经 JSON Schema 校验后的入参对象。
- `context`：执行上下文：
  - `cwd`：当前会话的工作目录（**始终是会话目录，不是插件目录**）。
  - `sessionId`：当前会话 ID。
  - `dataDir`：该插件的持久化数据目录，插件可在此读写自己的状态文件。

### 返回值

返回值可以是：

1. **字符串**：Runtime 自动包装为单个文本 `content`。
2. **Pi 工具结果对象**：

```js
{
  content: [
    { type: 'text', text: '...' },
  ],
  details: { /* 结构化补充信息，可省略 */ },
}
```

结果序列化后不能超过 1 MB。

## 完整最小示例

```js
// index.mjs
export async function execute({ toolName, context }) {
  if (toolName !== 'example_echo') {
    throw new Error(`Unsupported tool: ${toolName}`)
  }
  return {
    content: [{ type: 'text', text: `workspace: ${context.cwd}` }],
    details: { sessionId: context.sessionId },
  }
}
```

对应清单见本文开头的完整示例。仓库还提供一个只读示例插件 [`examples/local-plugins/project-package-info`](../examples/local-plugins/project-package-info)，可直接参考安装。

## 约束与限制

| 项 | 限制 |
| --- | --- |
| 清单大小 | 最多 256 KB |
| 目录文件数 | 最多 512 个文件 |
| 目录总大小 | 最多 20 MB |
| 符号链接 | 目录及清单、入口均不允许包含符号链接 |
| 工具数 | 每个插件 1-32 个 |
| 执行超时 | 120 秒，超时终止 Worker |
| 结果大小 | 序列化后最多 1 MB |
| Worker 内存 | 旧生代上限 128 MB |
| 附加文件（`plugin_create`） | 最多 64 个 |

路径安全：`entry` 及附加文件路径必须是插件目录内的相对路径，Runtime 会拒绝绝对路径、`..` 逃逸，并校验入口解析后仍位于目录内。

## 安装与验证

安装前 Runtime 会做**不执行代码的静态预检**（`inspect`）：

1. 解析并校验清单与每个工具的 JSON Schema。
2. 校验 `entry` 指向目录内普通文件。
3. 检查工具名与内置/已装插件冲突。
4. 扫描目录，计算内容 SHA-256 digest，检查文件数、体积与符号链接。

预检结果 10 分钟内有效。安装时 Runtime 会重新计算 digest，若目录在预检后发生变化则拒绝并要求重新检查；复制后再次比对 digest 后才落盘。因此「预检通过 → 检查后改文件 → 直接安装」会被拦截。

安装位置：`<dataDir>/plugins/<id>/<version>/`（`<dataDir>` 即 `~/.pisper/agent`，可经 `PISPER_AGENT_DIR` 更改）。

## 让 Agent 创建插件

在「完全访问」执行模式下，可让 Agent 用内置 `plugin_create` 结构化生成插件，它复用与「插件」页面完全相同的清单、Schema、路径、体积与工具名冲突校验。生成的源码固定写入全局 `<dataDir>/plugin-sources/<plugin-id>`，不属于某个项目。

开发者用 `plugin_create` 时注意：它接收的是 `id`、`name`、`tools`、`entryCode` 与可选 `files`，等价于手写清单 + 入口后走同一条预检安装链。生成与安装失败时，只清理内容与本次生成一致的文件，不覆盖已有源码或已装插件。

## 安全边界

- 插件代码运行在独立 Worker 中，但**不是操作系统沙箱**。它仍可使用当前系统用户有权访问的文件、网络与 Node.js 模块。
- 第三方插件固定标记为**高风险**，只在会话「完全访问」执行模式下提供给 Agent。
- 插件执行期间不能卸载；取消 Agent 调用或超时会终止 Worker。
- 请只安装来源可信、已审阅的代码，不要在插件中硬编码凭据。

## 当前不支持

本版本为本地插件 MVP，暂不支持：插件市场、npm/Git 下载、自动更新、原生模块、生命周期脚本、Provider 注入、TUI UI、覆盖内置工具。
