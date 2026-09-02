# Team Mode Playbook

> Pisper 团队模式（lead + subagent）的协作红线，来自一次真实 team 会话：5 项 UI 优化，lead 委派了 4 个 subagent（拆分重构 / 性能 / 无障碍 / UX）。本手册记录的每一次事故都真实发生过，代价包括单个卡死 agent 空耗 45+ 分钟、中断产生损坏工作区、以及中断后残留消息把工作流拖回未完成状态。

## 背景：真实事故清单

| # | 事故 | 代价 | 对策章节 |
| --- | --- | --- | --- |
| 1 | 重构 agent 后期陷入反复全量验证循环，45+ 分钟无产出；lead 只能靠文件 mtime 判断是否卡死，最终中断接管 | 45+ 分钟空耗 | 升级与接管协议 |
| 2 | 中断恰好落在多文件重构中间，产生「主文件 import 已删除子文件」的损坏工作区；lead 从 HEAD 恢复后以守卫兼容方式重做 | 返工 | 中断修复 SOP |
| 3 | 中断后此前发送的消息仍把工作流拉回未完成状态，阻塞目标完成，最后靠重新 spawn 收尾 | 额外等待与阻塞 | 升级与接管协议 |
| 4 | 各 agent 各自反复跑全量 `npm test` / `npm run build`，重复消耗大量时间 | 大量重复时间 | 验证分层策略 |
| 5 | 仓库有大量 `readFile + assert.match` 源码守卫测试，重构极易踩坏；必须在 spawn 消息里列明红线 | 回归风险 | 源码守卫测试红线 |

## 任务分解与文件所有权

- 分解任务前先枚举改动涉及的文件清单。spawn 消息里的 **文件白名单必须穷尽**：所有要新建、修改、甚至需要读取上下文的文件都要列出。
- **重叠文件 = 冲突。** 两个 subagent 的白名单若有交集，要么合并为一个任务，要么由 lead 串行执行；绝不允许并行写同一文件。
- 多文件拆分重构（例如把组件拆成主文件 + 多个子文件）必须作为一个整体任务交给单个 subagent。拆给多个 agent 后，任何一个中断点都可能留下「主文件 import 已删除、子文件未建好」的半成品状态。
- subagent 执行中发现需要动白名单之外的文件：先停下来上报 lead，由 lead 批准扩充白名单后再动，不要自行修改。
- lead 维护一张全局文件所有权表（subagent × 文件），spawn 前交叉核对无交集。

## 验证分层策略

验证分两层执行，重命令只在 lead 的集成层跑一次：

| 层 | 执行者 | 允许的命令 | 说明 |
| --- | --- | --- | --- |
| 定向自验 | subagent | `npx tsx --test runtime/tests/<file>.test.mjs` 等定向测试 | 只跑与本任务白名单文件相关的测试 |
| 集成闸门 | lead | `npm run check`、`npm test`、`npm run build` | 所有 subagent 交付后统一跑一次 |

定向测试示例（可一次传多个文件）：

```bash
npx tsx --test runtime/tests/agent-avatar-rendering.test.mjs
npx tsx --test runtime/tests/agent-avatar-rendering.test.mjs runtime/tests/api-handler-routing.test.mjs
```

- 禁止 subagent 跑 `npm test` / `npm run check` / `npm run build`：这些命令单次耗时以分钟计，多个 agent 反复跑只会重复消耗，不产生新信息。
- 禁止全量验证循环：**同一命令失败两次且无进展，立即停止并上报 lead**；不要第三次换参数重试，更不要「顺便」跑全量验证。
- lead 判断「卡死」用可观察信号：文件 mtime、增量产出、subagent 汇报。较长时间（数十分钟量级）没有文件变更也没有有效汇报的 subagent，进入升级与接管协议。

## 源码守卫测试红线

本仓库 `runtime/tests/*.test.mjs` 中有大量**源码守卫测试**：它们 `readFile` 某个源文件，用 `assert.match` / `assert.equal` 把历史修复结论逐字钉死在源码里。例如 `runtime/tests/agent-avatar-rendering.test.mjs` 断言 `src/components/AgentStatusAvatar.tsx` 不得包含 `<filter` / `<feDropShadow`，且必须包含特定 `className` 与 Tailwind arbitrary 值。

这些测试保护的是已知的回归结论，重构是最容易踩坏它们的场景。因此：

- 清点工具：`node scripts/list-source-guards.mjs` 可列出当前守卫测试钉住的源码模式。重构类任务的 spawn 消息必须附上守卫清单（至少附上与白名单文件相关的全部守卫条目）。
- 守卫测试**只读**：subagent 不得修改守卫测试，不得放宽任何断言。
- 发现守卫断言与当前代码不一致（过时断言，或重构必须改变被钉住的形式）：**上报 lead**，由 lead 决定是否更新守卫；不得自行修改测试。
- 守卫兼容的重构原则：所有被守卫钉住的源码形式（类名、属性、字符串字面量、调用形式）在重构中逐字保留；确需改动时先上报。

## 升级与接管协议

当 subagent 疑似卡死（长时间无文件变更、无汇报、或陷入验证循环）时，按顺序执行：

1. **发送收尾消息**：要求 subagent 在限定时间内交出当前状态——已改文件、未完成项、验证结果。
2. **等待一次**（约 30 秒）。
3. **观察可观察信号**：文件 mtime、新增产出、测试输出。若仍无变化且无有效交接，判定卡死。
4. **中断该 subagent。** 中断前就要有心理预期：**中断后工作区可能处于中间态**（import 已删但子文件未处理、文件写了一半等）。
5. **lead 接管**：先按「中断修复 SOP」把工作区恢复到稳定状态，再决定由 lead 串行完成还是重新 spawn。

两条附加红线：

- 中断之后，**不要再向旧 agent 发送任何消息**。真实教训：中断后此前发送的消息仍会把工作流拉回未完成状态、阻塞目标完成。收尾一律靠重新 spawn 新 subagent 或 lead 直接接管。
- 重新 spawn 的消息必须写明：这是接管收尾任务、当前工作区状态、已恢复的文件清单。

## 中断修复 SOP

中断卡死 subagent 后，lead 的标准修复流程：

1. **类型检查**：跑 `npm run typecheck`（或缩小范围的 `npx tsc --noEmit`），定位编译错误。
2. **定位悬空引用**：顺着错误找到指向已删除文件的 import、引用已删除导出的代码。
3. **恢复受损文件**：`git checkout HEAD -- <受损文件>`，把损坏文件一次性恢复回 HEAD；不要在半成品状态上打补丁。
4. **以守卫兼容方式重做**：对照 `node scripts/list-source-guards.mjs` 的守卫清单重新执行重构，所有被守卫钉住的模式逐字保留。
5. **定向测试确认**：`npx tsx --test runtime/tests/<相关>.test.mjs` 全绿。
6. 回到正常流程，由 lead 在集成层跑全量验证后再交付。

## spawn 红线消息模板

每个 subagent 的 spawn 消息按此模板书写；重构类任务必须附守卫清单。`⟨ ⟩` 内为占位说明，无对应条目时删除该行，但「文件白名单」「红线」「验证要求」「汇报格式」四节不可省略：

```text
【目标】
⟨一段话说明本任务要完成什么、验收标准是什么⟩

【文件白名单】（只允许新建/修改下列文件；如需改动名单外文件，先上报 lead）
⟨- src/features/xxx/…⟩
⟨- runtime/tests/xxx.test.mjs（定向测试文件：只允许运行，不允许修改）⟩

【红线】
- 守卫测试只读：不得修改或放宽 runtime/tests/*.test.mjs 中的任何断言。
- 发现守卫断言与代码不一致时，上报 lead，不得自行改测试。
- 重构须逐字保留所有被守卫钉住的模式（类名/属性/字符串字面量）。
⟨重构类任务附守卫清单：node scripts/list-source-guards.mjs 的输出，或其中与本任务相关的子集⟩

【验证要求】
- 只运行与本任务相关的定向测试：npx tsx --test runtime/tests/xxx.test.mjs
- 不要运行 npm test / npm run check / npm run build；全量验证由 lead 在集成阶段执行。
- 同一命令失败两次且无进展，立即停止并上报 lead。

【汇报格式】
- 改动的文件清单（逐条对照白名单）
- 执行过的验证命令及结果
- 未完成项与已知风险
- 发现的守卫冲突或名单外改动需求（如有）
```
