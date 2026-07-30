# Pisper TUI

Pisper 的 Rust + Ratatui 终端客户端。它复用现有 Node SEA sidecar、HTTP/SSE API、会话、Tools、Skills、MCP、权限和审批链，不包含第二套 Agent runtime。

## 本地运行

```bash
npm run tui:dev
```

在指定工作目录启动：

```bash
npm run tui:dev -- --cwd /path/to/project
```

构建完整发行目录：

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

将该目录加入 `PATH` 后，可在项目目录直接运行。普通启动始终创建空会话：

```bash
pisper
```

只有显式调用 `resume` 才恢复当前工作目录最近的历史会话：

```bash
pisper resume
pisper resume --cwd /path/to/project
```

诊断 sidecar、鉴权和能力目录：

```bash
pisper doctor
```

## 启动画面

每次启动 TUI 时，主区域都会先显示三行 `PISPER` 终端线条字标和 `›_` 光标。宽度不足 40 列或高度不足 6 行时自动切换为紧凑字标，避免裁切；开始交互后，启动标识自动让位给正常消息流。

## 交互

模型提供 reasoning token 时，TUI 会在回答之前实时渲染 `THINK`：活跃状态使用由 `thinking_patch` 推进的终端 spinner，展开最近 5 个视觉行；完成后收束为 2 行。模型或上游接口未提供 reasoning 时只显示思考状态，不生成或伪造过程文本。

TUI 直接接收 runtime 的原始 Text、Thinking 和 Tool 事件，不对本地聊天正文设置脱敏缓冲。供应商合并的大 delta 会进入积压感知 typewriter，以 24 ms 条件帧平滑追赶；积压清空后停止动画重绘。会话和 Subagent 内容按原文保存在本机数据目录，请勿公开分享其中的个人数据或凭据。

- 输入 `/`：呼出 Tools、已启用 Skills 和 TUI 命令的混合列表。
- `Enter`：提交消息或选中 Slash 候选。
- `Up` / `Down`：浏览 Slash 候选或会话选择器。
- `PageUp` / `PageDown`：滚动会话。
- `Ctrl+C`：运行中终止当前 Agent；空闲时退出。
- `Y` / `N`：处理 runtime 发出的 Tool 审批。

### 执行模式

输入 `/mode` 查看当前模式，并使用以下命令调整当前会话：

```text
/mode read-only
/mode workspace
/mode full-access
```

- `read-only`：只开放低风险分析工具，不允许修改项目。
- `workspace`：文件工具限制在当前工作目录；读取直接执行，文件修改和每条 Shell 命令都需要用户授权。
- `full-access`：允许本机文件和网络访问，Shell 以当前系统用户权限运行且不再逐次请求授权。该命令代表用户明确要求提升当前会话权限。

### Tool

输入 `/` 后，前缀 `T` 表示 Tool。选中后在命令后补充任务：

```text
/read README.md
/bash npm test
/web_search Pisper latest release
/mcp_pencil_get_editor_state_f9837b9b 读取当前 Pencil 画布状态
```

TUI 会从首个 Slash token 解析精确 Tool ID，并通过 `/api/chat` 的 `requestedToolNames` 结构化字段请求 runtime 激活对应 Schema。它不会在界面层直接执行 Tool，也不会自行构造 Tool 参数。Agent 仍负责生成参数和发起调用，runtime 继续执行 Tool 开关、`/mode`、工作区、沙箱及审批策略。

Conversation 视图会按终端高度限制当前活动区：Thinking 保持在 Tool 组之前，Tool 只显示最近调用，较早项目合并为计数摘要；每条 Tool 记录按可用列宽裁切为单个视觉行。完整事件仍可通过 `/events` 查看。

### Skill

输入 `/` 后，前缀 `S` 表示当前 runtime 中已启用且允许调用的 Skill。选择后添加任务，例如：

```text
/skill:docs-search 查找 Tauri updater 的签名要求
```

实际名称以 Slash 列表为准。Skill 命令作为显式用户指令发送给 Agent，runtime 按需加载对应 `SKILL.md`、脚本和参考资料。Skill 内触发的 Tool 仍走同一权限链。

### 内置命令

- `/new`：创建会话。
- `/sessions`：切换会话。
- `/events`：打开事件账本。
- `/chat`：返回会话。
- `/model`：显示当前模型。
- `/mode`：显示当前执行模式和参数。
- `/quit`：退出。

## 开发连接

默认情况下，开发版会运行仓库中的 `server/sidecar.mjs`。发行版会查找与 `pisper` 同目录的 `pisper-sidecar` 和 `sidecar-runtime/`。

也可以连接已经运行的隔离 sidecar：

```text
PISPER_TUI_URL=http://127.0.0.1:<port>
PISPER_TUI_TOKEN=<desktop token>
```

或指定 sidecar 可执行文件：

```text
PISPER_SIDECAR_PATH=/path/to/pisper-sidecar
PISPER_APP_ROOT=/path/to/sidecar-runtime
```
