# Pisper TUI

![Pisper TUI 启动画面](https://ling-kong-ran.github.io/pisper/shots/cli.png)

![Pisper TUI Chat 与实时活动](https://ling-kong-ran.github.io/pisper/shots/cli-chat.png)

[项目主页](https://ling-kong-ran.github.io/pisper/) · [GitHub 仓库](https://github.com/ling-kong-ran/pisper)

[English guide](https://github.com/ling-kong-ran/pisper/blob/release/src-tui/README.en.md)

## 安装终端命令

需要 Node.js 20+。安装后直接运行 `pisper`：

```bash
npm install -g pisper --progress=true --foreground-scripts
pisper
```

使用 npm 镜像时，平台包会走同一个 registry。`--progress=true` 显示 npm 下载进度，`--foreground-scripts` 显示本地验签和解压阶段；需要查看请求和缓存明细时再增加 `--loglevel=info`：

```bash
npm install -g pisper --registry=https://registry.npmmirror.com --progress=true --foreground-scripts
```

安装 Pisper 桌面版后，也可以打开 **设置 → 应用更新**，安装、修复或卸载 `pisper` 命令。Pisper 将可执行文件安装到当前用户目录并管理对应的 `PATH` 项，不需要管理员权限。

通过桌面版安装或卸载后需要完全重启终端宿主：Windows Terminal 需退出全部窗口后重开，IDE 内置终端需重启 IDE。

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

## 发行版更新

检查或更新 npm 安装的 Pisper：

```bash
pisper update --check
pisper update
```

命令使用当前配置的 npm registry。桌面版更新在 **设置 → 应用更新** 中管理。

## Provider 与可选 Web 配置

首次进入 TUI 后输入 `/provider` 并选择 Provider，即可编辑协议类型、Base URL 和 API Key。Base URL 会预填 Runtime 当前生效地址：未自定义时显示官方默认地址，已自定义时显示自定义地址；API Key 留空会保留已保存的密钥。使用 `Up`/`Down` 或 `Tab` 切换字段，使用 `Left`/`Right` 选择协议，`Enter` 保存，`Esc` 返回 Provider 列表或取消。API Key 使用独立掩码输入，粘贴时会移除空白字符，且密钥不会进入 composer、Slash 历史或 Runtime 的普通配置响应。已知 Provider 可直接输入 `/provider <id>` 跳过选择，例如 `/provider deepseek`；`/apikey` 保留为兼容别名，功能相同。

需要完整可视化配置时运行：

```bash
pisper web
```

该命令在默认浏览器打开本机 Provider 设置；浏览器使用期间需保持终端中的 Pisper 进程运行。**保存 Provider 配置**只保存当前连接，**设为默认 Provider**才会修改后续会话的默认模型。

Web 前端已安装时，可在 TUI 中输入 `/web` 再次打开配置页；尚未安装时先退出并运行 `pisper web`。

## 启动与会话

在项目目录运行：

```bash
pisper
```

普通 `pisper` 始终创建空会话，不会自动打开历史记录。显式使用 `resume` 会打开跨所有 workspace 的交互式会话列表；使用方向键选择，按 `Enter` 恢复，按 `Esc` 退出：

```bash
pisper resume
```

没有历史会话时，`pisper resume` 会提示后退出，不会创建新会话。恢复只使用会话已保存的工作目录，不会将其改写为 TUI 启动目录。进入会话后可通过 `/dir <目录>` 显式修改该会话的工作目录；相对路径以当前会话目录为基准。

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

- `Enter`：提交消息；Agent 运行期间提交的无附件文本会立即追加到当前 run，与桌面端使用相同的 Runtime `steer` 队列。带附件的消息会保留到当前 run 结束后发送，避免丢失附件。
- `/`：打开由 runtime Tools、已启用 Skills 和内置命令合并而成的 Slash 列表。
- `Up` / `Down`：移动 Slash、会话、模型、思考等级或文件选择器中的选中项。
- `Tab`：仅在 Slash 列表打开时补全当前候选。
- `Esc`：关闭当前选择器、清空 Slash 草稿，或从 Changes 返回 Chat。
- `↑` / `↓`：逐行翻阅消息；`PageUp` / `PageDown`：每次翻阅 8 行。历史消息按需分页加载，内存中最多保留 160 条，空闲后回收到最近 80 条，避免长会话持续占用内存。
- `Home` / `End`、`Left` / `Right`、`Backspace` / `Delete`：编辑 composer 草稿。
- `Ctrl+C`：Agent 运行或等待审批时中止当前 run（若 Agent 卡住未在超时窗口内响应，Runtime 会强制结束该次运行）；运行中再次 `Ctrl+C` 强制退出 TUI；空闲时退出 TUI。

最底部状态栏整体左对齐。会话运行期间，最左侧使用单个呼吸灯动画（亮度由暗到亮再到暗循环，例如 `○ ◔ ◑ ◕ ●`），不再显示 `Thinking`、`Responding` 等阶段文字；之后依次显示执行模式、模型、思考等级和指标，例如 `●  [full-access]  gpt-5.6-sol  high  ·  88M  ·  cache 79%`。队列和审批位置追加在指标之后，数据按会话隔离。

TUI 默认使用终端真彩色，并在 `TERM` 表明仅支持 256 色时自动降级。可用 `PISPER_TUI_THEME=ansi256` 强制 256 色，或用 `PISPER_TUI_THEME=monochrome` / `NO_COLOR=1` 使用单色模式。设置 `PISPER_TUI_REDUCED_MOTION=1` 会关闭装饰动画并立即显示完整流式文本；翻阅历史消息时也会暂停逐字揭示。

长文本或多行 bracketed paste 会在 composer 中折叠为 `[Pasted text · …]`，但提交给 Agent 的仍是包含原始换行的完整文本。摘要可作为整体移动和删除。

## 内置命令

| 命令 | 作用 |
| :--- | :--- |
| `/init` | 分析当前项目并创建或完善 workspace 根目录的 `AGENTS.md`。 |
| `/new` | 在 TUI 启动时的 workspace 新建空会话；运行期间不可执行。 |
| `/sessions` | 打开跨所有 workspace 的历史会话选择器；按 `Enter` 恢复，运行期间不可切换。 |
| `/dir <目录>` | 显式修改当前会话的工作目录；相对路径以当前会话目录为基准，运行期间不可修改。 |
| `/changes` | 查看 Git 或 SVN 工作区改动；改动页中 `R` 刷新、`C` 提交、`P` 推送 Git、连续按两次 `V` 撤销；SVN 没有 Push。 |
| `/changes commit <message>` | 使用指定 message 提交当前 Git/SVN 工作区改动。 |
| `/chat` | 返回 Chat 消息流。 |
| `/model` | 打开模型选择器并切换当前会话模型；只显示已配置 Provider 的模型，运行期间不可切换。 |
| `/thinking` | 刷新并打开当前模型支持的思考等级；不支持或加载失败时显示原因并可重试，运行期间不可切换。 |
| `/provider` | 编辑 Provider 的协议类型、当前生效 Base URL 和掩码 API Key；`/provider <id>` 可直达指定 Provider，`/apikey` 为兼容别名。 |
| `/web` | 使用已安装的 Web 前端，在默认浏览器打开本机认证配置页。 |
| `/compact` | 立即摘要较早上下文；仅用于已有可压缩历史的空闲会话。 |
| `/attach` | 打开 workspace 文件选择器。 |
| `/mode` | 显示当前执行模式和可用参数；运行期间也可随时调整。 |
| `/mode read-only` | 只开放低风险分析工具，不允许修改项目。 |
| `/mode full-access` | 允许本机文件、网络和 Shell 完整访问。 |
| `/quit` | 退出 TUI。 |

命令候选会根据前缀匹配和本机使用频率排序。`Tab` 只补全，不会执行 Tool；选择内置命令后按 `Enter` 才执行。

`/init` 会让 Agent 先检查项目结构、命令和约定，再生成项目专属的 `AGENTS.md`，而不是写入固定模板。已有文件会在保留有效说明的基础上谨慎更新，且该任务不会修改其他项目文件。`read-only` 模式下不能运行；`full-access` 模式下以当前系统用户权限执行。命令完成后使用 `/new`，可让新会话从启动时加载生成的项目说明。

Agent 创建结构化 Plan 后，TUI 会在消息流和 Composer 之间原位显示步骤、负责人、依赖和状态；窄终端只保留当前项，顶部细分隔显示 `Plan 完成数/总数`。

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

TUI 只把首个 Slash token 作为结构化 `requestedToolNames` 请求发送给 runtime。Agent 仍负责生成参数和调用 Tool，Slash 选择不会绕过 Tool 开关或当前执行模式；附件仍限制在当前 workspace 内。

Skill 示例：

```text
/skill:docs-search 查找 Tauri updater 的签名要求
```

Skill 名称以当前 Slash 列表为准。runtime 会按需加载对应的 `SKILL.md`、脚本和参考资料，Skill 内触发的 Tool 仍走相同权限链。

`read-only` 模式不会开放写入或 Shell 能力；`full-access` 代表用户明确允许以当前系统用户权限完整访问。

## 本地开发

从源码构建 TUI 需要 Rust 1.88 或更高版本。

从源码启动当前目录：

```bash
npm run tui:dev
```

指定工作目录：

```bash
npm run tui:dev -- --cwd /path/to/project
```

开发版默认运行仓库中的 `runtime/sidecar.mjs`。也可以连接已经运行的隔离 sidecar：

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
