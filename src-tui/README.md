# Pisper TUI

![Pisper TUI 启动画面](../docs/shots/cli.png)

[English guide](./README.en.md)

## 安装终端命令

安装 Pisper 桌面版后，打开 **设置 → 终端**，安装、修复或卸载 `pisper` 命令。Pisper 将可执行文件安装到当前用户目录并管理对应的 `PATH` 项，不需要管理员权限。

安装或卸载后需要完全重启终端宿主：Windows Terminal 需退出全部窗口后重开，IDE 内置终端需重启 IDE。

也可以从源码构建完整发行目录：

```bash
npm run sidecar:sea
npm run tui:package
```

产物位于：

```text
release/tui/pisper-<version>-<platform>-<arch>/
├── pisper[.exe]
├── pisper-sidecar[.exe]
└── sidecar-runtime/
```

## 启动与会话

在项目目录运行：

```bash
pisper
```

普通 `pisper` 始终创建空会话，不会自动打开历史记录。只有显式使用 `resume` 才恢复当前工作目录最近的会话：

```bash
pisper resume
pisper resume --cwd /path/to/project
```

指定其他工作目录创建新会话：

```bash
pisper --cwd /path/to/project
```

诊断 sidecar、鉴权和能力目录：

```bash
pisper doctor
```

诊断输出会列出 sidecar 连接来源、TUI 启动目录、sidecar runtime fallback 目录，以及该 workspace 的最近会话目录，便于定位目录继承问题。

## Composer 与消息流

- `Enter`：提交消息；Agent 运行期间提交的消息进入 FIFO 队列，并在当前 run 正常结束后依次发送。
- `/`：打开由 runtime Tools、已启用 Skills 和内置命令合并而成的 Slash 列表。
- `Up` / `Down`：移动 Slash、会话、模型、思考等级或文件选择器中的选中项。
- `Tab`：仅在 Slash 列表打开时补全当前候选。
- `Esc`：关闭当前选择器、清空 Slash 草稿，或从 Events 返回 Chat。
- `↑` / `↓`：逐行翻阅消息；`PageUp` / `PageDown`：每次翻阅 8 行。TUI 仅保留最新 100 条消息，避免长会话持续占用内存。
- `Home` / `End`、`Left` / `Right`、`Backspace` / `Delete`：编辑 composer 草稿。
- `Ctrl+C`：Agent 运行或等待审批时中止当前 run；空闲时退出 TUI。

长文本或多行 bracketed paste 会在 composer 中折叠为 `[Pasted text · …]`，但提交给 Agent 的仍是包含原始换行的完整文本。摘要可作为整体移动和删除。

## 内置命令

| 命令 | 作用 |
| :--- | :--- |
| `/init` | 分析当前项目并创建或完善 workspace 根目录的 `AGENTS.md`。 |
| `/new` | 在 TUI 启动时的 workspace 新建空会话；运行期间不可执行。 |
| `/sessions` | 打开历史会话选择器；跨 workspace 会话需要再次确认，运行期间不可切换。 |
| `/events` | 打开当前 TUI 进程的事件账本。 |
| `/chat` | 返回 Chat 消息流。 |
| `/model` | 打开模型选择器并切换当前会话模型；运行期间不可切换。 |
| `/thinking` | 刷新并打开当前模型支持的思考等级；不支持或加载失败时显示原因并可重试，运行期间不可切换。 |
| `/attach` | 打开 workspace 文件选择器。 |
| `/mode` | 显示当前执行模式和可用参数；运行期间也可随时调整。 |
| `/mode read-only` | 只开放低风险分析工具，不允许修改项目。 |
| `/mode workspace` | workspace 内读写直接执行；每条 Shell 命令需要审批。 |
| `/mode full-access` | 允许本机文件、网络和 Shell 完整访问。 |
| `/quit` | 退出 TUI。 |

命令候选会根据前缀匹配和本机使用频率排序。`Tab` 只补全，不会执行 Tool；选择内置命令后按 `Enter` 才执行。

`/init` 会让 Agent 先检查项目结构、命令和约定，再生成项目专属的 `AGENTS.md`，而不是写入固定模板。已有文件会在保留有效说明的基础上谨慎更新，且该任务不会修改其他项目文件。`read-only` 模式下不能运行；`workspace` 模式下写入直接执行，无需审批。命令完成后使用 `/new`，可让新会话从启动时加载生成的项目说明。

Agent 创建结构化 Plan 后，TUI 会在消息流和 Composer 之间原位显示步骤、负责人、依赖和状态；窄终端折叠已完成项，底部保留 `Plan · 完成数/总数`。

## 附件

使用以下任一入口打开附件选择器：

- composer 为空时输入 `+`
- `Ctrl+O`
- `/attach`

文件选择器操作：

- `Up` / `Down`：选择文件或目录。
- `Enter` / `Right`：进入目录或添加文件。
- `Left`：返回上级目录，但不会越过当前 workspace。
- `Delete`：移除当前已选附件。
- `Esc`：关闭选择器并保留 composer 草稿。

安全限制与桌面端一致：最多 8 个文件，单文件不超过 10 MiB，总计不超过 20 MiB；只允许 workspace 内的图片、UTF-8 文本/代码和支持的文档。仅当当前模型明确支持图片输入时，图片才会作为视觉输入发送。

## Tool 与 Skill

输入 `/` 后，前缀 `T` 表示 runtime Tool，前缀 `S` 表示已启用且允许模型调用的 Skill。

Tool 示例：

```text
/read README.md
/bash npm test
/web_search Pisper latest release
/mcp_pencil_get_editor_state_f9837b9b 读取当前 Pencil 画布状态
```

TUI 只把首个 Slash token 作为结构化 `requestedToolNames` 请求发送给 runtime。Agent 仍负责生成参数和调用 Tool，Slash 选择不会绕过 Tool 开关、执行模式、workspace 边界或审批。

Skill 示例：

```text
/skill:docs-search 查找 Tauri updater 的签名要求
```

Skill 名称以当前 Slash 列表为准。runtime 会按需加载对应的 `SKILL.md`、脚本和参考资料，Skill 内触发的 Tool 仍走相同权限链。

## 审批

在 `workspace` 模式下，workspace 内的结构化文件读写直接执行，只有每条 Shell 命令需要逐次审批。审批面板临时替代 composer：

- `Y`：`Allow once`，仅允许当前请求。
- `N`：拒绝。
- `Esc`：拒绝。
- `Ctrl+C`：拒绝待执行请求并中止当前 run。

`read-only` 模式不会开放写入能力；`full-access` 代表用户明确允许完整访问，它不是安全沙箱。

## 本地开发

从源码启动当前目录：

```bash
npm run tui:dev
```

指定工作目录：

```bash
npm run tui:dev -- --cwd /path/to/project
```

开发版默认运行仓库中的 `server/sidecar.mjs`。也可以连接已经运行的隔离 sidecar：

```text
PISPER_TUI_URL=http://127.0.0.1:<port>
PISPER_TUI_TOKEN=<desktop token>
```

或指定 sidecar 可执行文件和 runtime：

```text
PISPER_SIDECAR_PATH=/path/to/pisper-sidecar
PISPER_APP_ROOT=/path/to/sidecar-runtime
```

## 验证

```bash
npm run tui:test
npm run tui:check
cargo clippy --manifest-path src-tui/Cargo.toml --all-targets -- -D warnings
cargo build --manifest-path src-tui/Cargo.toml --release
```
