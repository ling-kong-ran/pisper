# Release 更新说明规范

本规范适用于统一发布流程生成的 Desktop、Runtime、TUI 和 App GitHub Release。生成器为 `scripts/release-notes-format.mjs`，各渠道通过既有准备脚本调用；发布前使用 `scripts/validate-release-notes.mjs` 校验正文和客户端清单。

npm 安装器目前没有独立 GitHub Release，继续使用现有 npm 发布流程和独立版本，不因本规范新增发布渠道。历史 Release 不批量重写，旧版说明继续兼容显示。

## 格式

GitHub Release 标题为 `Pisper 渠道名 版本号`，渠道名统一使用 `Desktop`、`Runtime`、`TUI`、`App`。版本号不重复包含渠道标签前缀，完整标签放在正文版本信息中。

正文固定包含以下顺序：

1. 版本标题与版本信息。
2. 更新内容，按固定类别顺序展示。
3. 升级说明。
4. 完整变更与构建源提交链接。

示例：

```markdown
## Pisper Runtime 1.2.3

- 发布日期：2026-10-14（北京时间）
- 发布标签：`runtime-v1.2.3`
- 对比基线：`runtime-v1.2.2`

## 更新内容

### 新增功能

- feat(chat): 新增消息搜索

### 优化改进

- perf(runtime): reduce repeated reads

### 问题修复

- fix: 恢复连接

## 升级说明

- 未提供额外的升级说明，请结合下方完整变更确认影响。

## 完整变更

- [查看完整变更](https://github.com/ling-kong-ran/pisper/compare/runtime-v1.2.2...runtime-v1.2.3)
- [构建源提交](https://github.com/ling-kong-ran/pisper/commit/0123456789abcdef0123456789abcdef01234567)
```

示例版本和提交仅演示格式，不表示存在对应发布。

## 分类与内容

| 顺序 | 分类     | 提交前缀                                               |
| ---- | -------- | ------------------------------------------------------ |
| 1    | 新增功能 | `feat`、`feature`                                      |
| 2    | 优化改进 | `perf`、`refactor`、`style`、`ui`、`ux`、`enhancement` |
| 3    | 问题修复 | `fix`、`bugfix`                                        |
| 4    | 其他变更 | 其他前缀和无 Conventional Commit 前缀的标题            |

- 空分类不显示，已有分类保持上述顺序。分类不是单独允许发布的依据，版本仍受原有实质性变更检查约束。
- 提交说明保留原语言和类型前缀，避免自动翻译或生成无法核实的功能描述。
- `chore(release):` 和 `chore(release-渠道):` 版本记账提交不进入更新条目。
- 渠道归属沿用既有路径规则；删除文件的提交也应纳入说明，不能因为只有删除操作就漏掉。
- 提交中的 Markdown 特殊字符和 HTML 标记按普通文字转义，不能改变正文结构。
- 没有找到该渠道提交说明时，在“其他变更”中明确写明未检索到记录并要求核对完整变更，不推断这是安全、无影响或纯维护升级。

## 升级说明

普通版本没有专门说明时使用固定的中性提示，不自动声称“不需要操作”“无兼容性影响”或“全部平台已验证”。

提交标题包含 Conventional Commit 的 `!` 标记时，增加不兼容变更提示，提醒查看对应提交。该提示只识别标题标记，不表示已经完成全部升级风险分析；具体影响仍需作者写清在提交中。

App 的 Android 已签名 APK 与 iOS 未签名构建安装说明集中放在这一节。产物签名策略变化时，必须同步修改说明与工作流，不能继续沿用旧安装承诺。

## 日期与链接

- 发布准备阶段统一按 `Asia/Shanghai` 生成 `YYYY-MM-DD` 日期，正文明确标注北京时间。
- 对比基线使用所属渠道准备脚本选出的上一个标签；旧版本跨渠道兼容回退规则保持不变。
- 首次发布显示“首次发布”，完整变更链接指向当前标签的提交历史。
- 非首次发布使用上一标签到本次标签的 compare 链接。
- 构建源链接指向经 Git 解析的不可变提交，不使用可移动的分支名。
- 仓库名称来自 `GITHUB_REPOSITORY`，本地默认 `ling-kong-ran/pisper`。

## 生成与发布校验

| 渠道                  | 生成入口                          | 正文及客户端清单                                                           |
| --------------------- | --------------------------------- | -------------------------------------------------------------------------- |
| Desktop、Runtime、TUI | `scripts/prepare-release.mjs`     | `release-body.md`；Desktop 与 Runtime 同步生成 `public/release-notes.json` |
| App                   | `scripts/prepare-app-release.mjs` | `release-body-app.md` 与 `docs/latest-app.json`                            |

TUI 不覆写桌面客户端的 `public/release-notes.json`。其他渠道中，客户端清单的正文、日期、版本及渠道或标签必须与 Release 正文一致。

校验程序接收正文文件、渠道、版本号和完整标签：

```text
node scripts/validate-release-notes.mjs release-body.md runtime 1.2.3 runtime-v1.2.3
node scripts/validate-release-notes.mjs release-body-app.md app 1.2.3 app-v1.2.3
```

上述命令只用于已有暂存产物的校验，不创建版本或标签。正文和对应 JSON 清单必须位于发布准备阶段的目录结构中。

发布工作流在任何 Git 或 GitHub 发布写操作之前校验下载的正文，校验失败即停止。Release 标题直接使用校验程序输出，创建草稿和公开发布使用同一份正文文件。

不要通过 GitHub 的自动生成说明按钮替换正文。`.github/release.yml` 只提供备用分类，不能代替统一模板、升级说明和发布前校验。

官网更新日志识别新分类并兼容旧 `What's Changed` 条目；无标准提交前缀的更新保留在对应分类。更新器原有 Markdown 和历史 HTML 说明兼容逻辑保持不变。

## 验证

`runtime/tests/release-notes-generation.test.mjs` 已接入 `npm test` 的实际发现入口。测试在临时 Git 仓库内运行真实准备脚本，覆盖四个渠道、首次发布、删除文件、分类顺序、元数据与版本记账过滤，以及客户端清单的一致性。

发布闸门测试执行工作流中的发布 Shell，使用隔离的 Git/GitHub 命令拦截器，确认不合规正文不会到达提交、标签或发布操作。测试不调用真实 GitHub 发布接口。

修改模板后运行相关发布测试、完整 `npm test` 和 `npm run check`，检查官网分类展示。模板或发布说明规范的维护不自动授权创建新版本、改写历史标签或编辑既有 Release。
