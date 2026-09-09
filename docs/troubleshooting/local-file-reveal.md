# 本地文件链接点击无反应

`local-reveal.log` 不存在，不能单独证明是文件管理器故障。v0.5.57 只在原生命令执行完成后写日志：未进入命令、系统调用尚未返回、日志写入失败，都可能没有文件。

## 在用户现有安装上收集信息

把仓库的 `scripts/diagnose-local-reveal.ps1` 发给用户。让用户保持出问题的 Pisper 运行，再点击一次出问题的链接，在该脚本所在目录打开 PowerShell 并运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\diagnose-local-reveal.ps1
```

如需同时检查链接中的目标是否存在，可以传入原始文件路径（不要带 Markdown 的方括号、反引号或行号）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\diagnose-local-reveal.ps1 -TargetPath 'C:\Users\Administrator\generated\visuals\chibi_character_replacement_corrected.gif'
```

脚本末尾输出 `Report saved to: ...txt`。收集该报告，并让用户复制消息中原始的链接文本，以及点击后出现的提示。脚本读取运行中的桌面程序路径、文件版本、进程账户、日志位置、最近 40 行 reveal 日志和目标路径状态；不读取会话、凭据、进程命令行，也不会重启 Pisper 或打开文件管理器。写入探针使用临时文件，关闭后自动删除，不创建缺失的应用日志目录。

比较 `ProductVersion` 和桌面组件版本，不能用 Runtime 版本代替。还要比较 `Process owner` 和 `Current user`：以另一账户运行的 Pisper 会使用该账户的日志目录。

## 日志位置

Windows 的主日志：

```text
%LOCALAPPDATA%\com.lingkongran.pisper\logs\local-reveal.log
```

在本次诊断改动构建后的版本中，主日志无法写入时还会尝试：

```text
%TEMP%\com.lingkongran.pisper\logs\local-reveal.log
```

备用日志路径和 `request` 入口记录不属于 v0.5.57；脚本可在旧版本上使用，但不能要求旧版本生成这些记录。

| 证据 | 可以判断什么 |
| --- | --- |
| `request desktop=... pid=... path=...` | 已到达 Rust 命令，记录了原生版本和请求路径 |
| 只有 `request`，没有结果 | 操作尚未返回，或结果日志写入失败 |
| `log-fallback:` | 主日志写入失败，后面是主路径和原始错误 |
| `resolve-failed` | 路径解析失败，且无合适祖先目录可回退 |
| `reveal-ok`、`open-dir-ok`、`ancestor-open-ok` | 系统调用接受了请求，不证明窗口已经出现或路径已被选中 |
| 页面错误包含 `local-reveal:unavailable` | 点击时没有可用的桌面桥接 |
| 页面错误包含 IPC 权限错误 | 请求在进入 Rust handler 之前被拒绝，此时不会有入口日志 |
| 页面错误包含 `local-reveal:timeout` | 前端停止等待结果；不会自动重发，系统操作仍可能稍后完成 |

本次 UI 改动会在错误时显示可复制的原因和“复制路径”按钮；成功提示用“已请求在文件管理器中显示”，不再把系统调用返回成功表述成窗口已出现。

## 开发者复验

真实浏览器点击测试使用隔离的无头 Chromium/Edge，不连接或修改正在运行的 Pisper：

```bash
node scripts/smoke-local-reveal.mjs --baseline
node scripts/smoke-local-reveal.mjs
npx tsx --test runtime/tests/local-path-reveal.test.mjs runtime/tests/markdown-link-rendering.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml
```

`--baseline` 编译 v0.5.57 的 Markdown 源码，复现桥接缺失或迟到时链接退化成 `span`、无请求、无提示。当前代码覆盖缺桥接提示、复制路径、迟到桥接恢复、正常调用、同步异常、IPC 拒绝、返回 false 和超时。浏览器测试以桩代替 Shell，不验证文件管理器窗口。

脚本自动查找已安装的 Playwright Chromium 或 Windows Edge；其他位置可以用 `PISPER_SMOKE_BROWSER` 指定浏览器可执行文件。
