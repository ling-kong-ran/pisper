# Windows CLI 被 Node 当作脚本读取

报错以 `pisper.exe:1 MZ` 开头，并包含 Node 的 `SyntaxError: Invalid or unexpected token`，说明某条 Node 执行路径正在把 Windows 可执行文件当作 JavaScript 读取。它不能单独证明具体调用程序、PATH 冲突或 Pisper 版本回归。升级之前正常，也需要比较升级前后的调用入口和实际命中的文件。

## 不知道调用入口时收集信息

把仓库的 [`scripts/diagnose-windows-cli.ps1`](../../scripts/diagnose-windows-cli.ps1) 和 [`scripts/diagnose-windows-cli.cmd`](../../scripts/diagnose-windows-cli.cmd) 放在同一个文件夹发给用户。双击 `.cmd` 后，通过原来出错的程序重现，等待诊断结束，把同目录的 `pisper-cli-report.json` 发回。重复运行会覆盖这份诊断报告。窗口会保留到按键关闭；写文件失败时可复制窗口中的 JSON。

脚本兼容 Windows PowerShell 5.1，无需安装 Node、npm 或其他工具；不要求升级 Pisper。若需检查用户当前 PowerShell 的临时别名和函数，在脚本所在目录的 PowerShell 中直接运行：

```powershell
& .\diagnose-windows-cli.ps1
```

看到 `Monitoring for 30 seconds` 后，通过原来出错的程序重现（如果操作可安全重复，可在窗口内再试一次），再把最后的 JSON 报告发回。无需知道那个程序内部的启动命令。仅做静态检查时传入 `-MonitorSeconds 0`；想留更多重现时间可传入 `-MonitorSeconds 60`。

若 PowerShell 的脚本执行策略阻止运行，可以用下面这条命令启动一次临时诊断，不改变持久执行策略：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\diagnose-windows-cli.ps1
```

双击 `.cmd` 和后一种命令都运行在新的 PowerShell 中，继承当前环境的 PATH，但看不到原 PowerShell 的临时函数、别名或 Profile 设置。两者不提权、不更改系统或用户的持久执行策略。报告中的 `shell*` 字段总是描述运行诊断脚本的 PowerShell，不能代表所有外部调用程序。直接运行 `.ps1` 默认只输出 JSON；需要保存时可加 `-OutputPath .\pisper-cli-report.json`。

脚本检查当前用户桌面应用管理的 CLI 文件头，直接运行一次该文件的 `--version`（最多等待 3 秒），读取 PowerShell 和 `where.exe` 的命令解析结果，并在所选时长内采样命令行提到受管 CLI 的进程（包括 Node 和 Electron）。它不会修改 PATH、注册表或安装内容，不会读取会话或脚本文件，也不会停止已有程序；只有自己创建的版本探针超时后会被停止。

候选进程命令行只在内存中匹配受管 CLI 路径，不写入报告。报告只包含固定字段、布尔值、固定状态值，以及捕获到的候选程序和其父程序可执行文件名；不包含完整路径、进程参数、Token、用户名或原始错误。整个过程包含文件检查、版本探针和进程采样，采样时长默认 30 秒、上限 60 秒；单次 Windows CIM 查询另设 1 秒操作超时，系统繁忙时总耗时可能略长。

## 解释报告

| 字段 | 含义 |
| --- | --- |
| `schemaVersion`、`windows` | 报告格式版本，以及是否在 Windows 运行；非 Windows 只返回未执行的默认字段。 |
| `managedCliExists`、`managedCliReadable`、`managedCliPeHeader` | 受管 CLI 是否存在、可读取、具有 `MZ` 与 `PE` 文件头。文件头正确不等于整个文件完整或可执行。 |
| `directVersionStatus`、`directVersionRecognized` | 直接启动结果：`not-run`、`success`、`nonzero-exit`、`timeout` 或 `unavailable`；版本文本是否符合 `pisper x.y.z` 格式。不会输出子进程原文。 |
| `shellInspectionSucceeded`、`shellFindsPisper`、`shellFirstIsManagedCli` | PowerShell 解析检查是否完成、是否有同名命令、优先项是否为受管 EXE。 |
| `shellHasAliasOrFunction`、`shellHasOtherExecutable` | 是否同时存在别名/函数或其他路径的可执行命令。存在不等于它就是报错来源。 |
| `whereInspectionSucceeded`、`whereFindsManagedCli`、`whereFindsOtherExecutable` | `where.exe` 检查是否完成、是否找到受管 CLI 或其他同名命令。它不会反映 PowerShell 函数和别名。 |
| `monitoringRequested`、`processSampleSucceeded`、`processSampleFailed` | 是否要求采样、是否至少成功一次、是否至少失败一次；后两者可同时为真。 |
| `processCommandLineUnavailable` | 返回的候选进程缺少可读取的命令行，需结合权限继续排查。查询本身也可能遗漏无权限的进程，因此为 `false` 不能证明读取完整。 |
| `managedCliMentionedByProcess` | 采样时发现其他程序的命令行提到受管 CLI 路径，排除受管 CLI 自身。仅提供调用线索；也可能是字符串参数或由 JavaScript 正常 `spawn` 的目标，不能单凭此字段确认误执行。 |
| `possibleInvokerExecutable` | 首次捕获的候选程序文件名，例如 `node.exe` 或 `Code.exe`，不含完整路径。 |
| `possibleCallerExecutable` | 候选进程的父程序文件名，例如 `some-editor.exe`。未捕获、父程序已退出或无法读取时为 `null`；它不是脚本名，也不保证就是最终调用入口。 |

若直接启动成功且版本格式正确，优先检查外部调用程序的启动设置和升级前后的命令解析差异。原生 `pisper.exe` 应直接执行；Node 程序可用 `spawn(exePath, args)`，而 `node exePath` 和 `fork(exePath)` 都会将 EXE 当作 JavaScript 读取。

采样是尽力而为：极短命的失败进程、通过相对路径或变量表达的路径、没有在命令行传递目标路径的嵌入式调用、权限不足或其他用户启动的进程都可能没有命中。`managedCliMentionedByProcess: false` 不能排除误调用。脚本不自动修改启动配置，也不把 PATH 修复或可执行文件头检查当作根因修复。

## 验证范围

修改脚本时需要检查 PowerShell 语法、非 Windows 与 `-MonitorSeconds 0` 路径、缺少受管文件、版本探针超时、命令解析冲突、CIM 不可用、命中与未命中的采样，以及报告中不含原始命令行或错误。Windows PowerShell 5.1 上的实际运行和外部程序故障复现仍需 Windows 环境验证；跨平台 PowerShell 的语法或替身测试不能替代这一步。
