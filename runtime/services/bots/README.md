# Bot 服务

状态（2026-11-05，v2.1）：**Bot = 一条对话 + 它派出去的子 agent**。对话自己做快的事（读、查、小改、短命令），长的、多步的、要批准的交给子 agent 在后台并行跑（每个 Bot 默认同时 3 个，多的排队），做完结果回到对话，对话用一句话汇报。运行时保证对话不阻塞：运行中发来的话插队马上处理，命令超过 10 秒或被打断时转后台，对话里不等批准。打开 Bot 就是打开这条对话（普通会话页）；渠道里叫它、定时让它做，都是往这条对话里说一句话。Bot 有自己的记忆与技能。不依赖 git。设计稿：本机 `~/.local/share/pisper-rebuild/bots-v2-dots.md`（参考 OpenAI dots、Grok Bot、Claude Duet；第 9 节是 v2.1）。执行节点上的 Bot 暂时仍是一个会话自己干（子 agent 还不能在执行节点上跑）。

主要文件：`bot-conversation.mjs`（对话的系统提示、状态快照、bot_* 工具输入的归一）、`runtime/tools/app/bot-conversation.mjs`（对话当经理用的 bot_* 工具）、`runtime/runtime/bot-background-commands.mjs`（命令转后台）、`bot-schedule-input.mjs`（给自己定时、问题的回答）、`bot-memory-service.mjs`（每个 Bot 的记忆）、`bot-skill-service.mjs`（每个 Bot 自己写的技能）、`bot-links.mjs`（Bot 之间的来往：来源标记、跳数与频率限制）、`bot-proactive.mjs`（主动巡看与每日简报：到点判断、交代、提示、上次时间）、`runtime/runtime/bot-decisions.mjs`（这一轮的来由、复盘的无头请求）、`bot-model.mjs`（校验与归一、系统提示、消息校验）、`bot-service.mjs`（注册表）、`bot-routing.mjs`（渠道分派）、`bot-notifications.mjs`（发回聊天的文字）、`environment-service.mjs` / `executor-hub.mjs` / `executor.mjs`（执行节点）、`runtime/http/routes/{bots,executors}.mjs`、`runtime/runtime/agent-runtime-facade.mjs` 里的 Bot 接线（`ensureBotSession`、`applyBotSettings`、`messageBot`、`botActivity`、`botForSession`）、`runtime/runtime/executor-tools.mjs`、`packages/pisper/lib/executor-node.mjs`；测试 `runtime/tests/bot-*.test.mjs`、`bots-frontend.test.mjs`。以代码、测试为准并同步修订本文。上线方式：实验功能 `bots`，默认关闭（见 `src/features/labs/README.md`）。

## 目标与非目标

参考 Grok Bot / Dots：用户可以有多个常驻的 Bot，每个有名字、头像、它负责什么（指令）、在哪台电脑的哪个目录里干活、权限上限、可用工具，以及谁能在聊天里叫它、什么时候定时找它。和 Bot 说话就像和同事聊天：一个长期对话，前文都算数，它说话时实时看得到，可以随时插话。

非目标（以后再说）：每个 Bot 的浏览器登录态；每个 Bot 绑定自己的渠道机器人账号；GitHub webhook 触发；多个 Bot 的群聊；巡看用到命令（CI 状态、git log）与联网（依赖公告）。

## 对话与子 agent（v2.1）

- **对话自己动手**：本机的 Bot（且有多 Agent 能力）建运行时时 `conversationOnly = true`：系统提示换成 `botConversationPrompt`（快的自己做、长的派子 agent、不为确认计划而停、“以后再做”用定时）。它按 Bot 的工具范围读、查、改、跑命令，外加当经理用的 `bot_*` 工具；通用的多 Agent、`ask_user`、定时、工作流、目标与浏览器自动化工具在这条对话上挡掉（`BOT_CONVERSATION_BLOCKED_TOOLS`），由 `bot_*` 代替。它写的技能它自己也带着。每轮在用户消息后附上状态快照（`botStateContext`：手上的子 agent 与后台命令、在等几个批准、可以挑的模型、可以找的别的 Bot、它记得的事、它会的技能），走附件上下文的标记，转录里不显示。
- **不阻塞**：运行中发来的话（应用里的 ↵、聊天、别的 Bot、问题的回答）一律插队（`queueSessionMessage` 把 steer 升为 interrupt；`messageBot` 对话在说话时排进去而不是回“在忙”，聊天来的话在这一轮说完后回到那个聊天），打断当前这一步马上处理；正在跑的命令不杀掉：对话的 bash 包了一层（`BotCommandJobs.wrap`），超过 10 秒或这一步被打断就转后台，工具结果写“已转后台”，跑完把结果作为内部消息交回对话（`injectParentNotice`），时间线里一行“后台命令：…”可停。对话里不等批准：权限服务对这条对话 `noWait`，要批准的操作直接退回并提示交给子 agent（子 agent 的审批另装，照常等）。
- **bot_* 工具**：`bot_job_start` / `bot_job_add` / `bot_job_stop`（派活、补充、停，只碰自己的活）、`bot_ask`（问题卡片，发出即返回，回答作为下一条消息说回对话）、`bot_schedule`（只能给自己定时：多少分钟后、某天某时、每天或每周；没人在场的那一轮里不能建；也能列、删自己的）、`bot_remember` / `bot_forget` / `bot_recall`、`bot_message`（找别的 Bot）。工具背后是 `botConversationRuntime`：每次调用时按这一刻的会话与设定执行。巡看结果与每日简报那一轮，派活与找别的 Bot 直接拒绝。对话这一轮说完（运行标记放下之后，`finishBoardCard` 记 `botTurnDue`，`streamPrompt` 收尾时排）跑 `afterBotTurn`：把回话交给在等的 Bot、回复发回聊天、清掉巡看标记、手上没活时整理记忆。不再有每轮额外的决定请求。
- **子 agent**：复用 `MultiAgentService`，父会话是 Bot 的对话。派活时工具清单按 Bot 的工具范围直接给（`toolNames`，不按对话此刻激活的工具继承），模型按 `bot_job_start` 挑（`resolvedModel`，不可用时退回档位、再退回 Bot 的模型），带上 Bot 的工作说明与记忆（`instructions` → 子 agent 的系统提示追加段），每个 Bot 同时最多 3 个（`parentLimit`，服务按父会话限流，多的排队）。标题另存（`title`），界面显示它。
- **批准与汇报**：子 agent 的批准挂在父会话（Bot 的对话）上，卡片出现在对话里与“需要你”；做完时完成通知唤醒对话（不在内存里时加载它再说进去），对话写一句汇报（需要的话接着派活）。
- **停与释放**：停掉对话这一轮不停它派出去的子 agent（`abortSession` 对 `conversationOnly` 不调 `abortParent`，也不拒绝子 agent 等着的批准）；改设定时还有子 agent 在跑就不释放运行时；还有子 agent 在跑的 Bot 不能删除。子 agent 只在内存里，Runtime 重启后正在跑的标为已中断。
- **时间线**：`GET /api/sessions/:id/agents` 列出会话派出的子 agent，`POST /api/sessions/:id/agents/:agentId/stop` 停掉一个。会话页把它们按开始时间插进对话（`withAgentEvents`，事件条目不算“正在写的回复”），一行写标题、状态、用时，可展开、可停；有活刚结束时重拉几次对话，让后台说的汇报马上出现。
- **招呼**：会话页认出 Bot 的对话时，转录最前面显示招呼（`src/features/bots/BotGreeting.tsx`，经 `public-greeting.ts` 懒加载）：头像、自我介绍、复述职责。对话还没有消息时带三张卡片：每日简报（设 `proactive.brief` 与 `briefAt`）、项目目录（已有职责时只设 `workspace.cwd`，不碰巡看；没写职责时先展开确认表单，讲清频率、第一遍马上跑和模型用量，用户确认并选好目录才写入“盯着这个项目”的工作说明，并按所选 `everyHours` 打开只读巡看）、聊天接入（跳到设置页的聊天分区）；执行节点上的 Bot 只有最后一张。有消息后卡片收起，招呼留在最前面，还有更早的消息未加载或被折叠时不显示。招呼只由设定算出来，不调用模型、不写进转录，也不进模型上下文；实验功能关闭或 Bot 已删除时退回普通空状态。
- **头像状态**：`botActivity` 在对话在说话或有子 agent 在干活时为 `running`，有批准或问题在等时 `needsYou`，另带 `working`（在做几件）与 `current`（正在做哪件）；侧栏名字后显示件数，悬停写“3 件在做 · 在做：…”。
- **记忆**：记忆引擎（`runtime/services/memory/`）里每个 Bot 一个独立空间（`bot:<id>`），`bot-memory-service.mjs` 是上面的薄适配，对外仍是一条条短句（每条 500 字），条数不设上限。只写自己的空间，默认只读全局（`readGlobalMemory`，设置页可关），Bot 之间互不可见：工具参数里没有空间字段，改、删前核对条目归属。来源：对话的 `bot_remember` / `bot_forget`（用户让它记住的、纠正、做完复盘）、设置页手动增改删（`/api/bots/:id/memory`）。记下就生效，不需要确认。可信度看这一轮的话是谁说的（`botSourceTrust`）：应用里的用户、渠道绑定的用户本人、问题卡片的回答算用户；定时任务不算用户；别的 Bot 和渠道里的其他人算外部（`tainted`，不进自动回忆）。每轮带画像（首轮和压缩后）和按这句话检索到的相关事实，想查更多用 `bot_recall`；子 agent 拿画像和与任务相关的事实。旧的 `memory.json` 第一次用到时导入一次后删除。整理归统一的后台整理。删除 Bot 时清掉整个空间。
- **技能（C4）**：子 agent 做完（`completed`）时 `injectAgentCompletion` 先调 `botAgentFinished`，另起一次无头复盘（`botReviewInstruction`，带上这件活的标题、交代、结果、已有技能与用户的纠正），只要 `{"skill": null}` 或一个技能；失败不影响汇报，同一个 Bot 的复盘按顺序跑。技能是标准 SKILL.md（`<dataDir>/bots/<botId>/skills/<name>/SKILL.md`，最多 40 个、正文 2 万字，同名即更新，原子写），由 `BotSkillService` 管。派活时把这个目录作为 `skillPaths` 交给多 Agent 服务，技能加载器（`SkillsService.createResourceLoader` 的 `extraSkillPaths`）把它们列进子 agent 的系统提示；不进技能管理页，也不给别的会话。对话的状态快照列出 `[你会的技能]`，派活时可以点名。设置页“它学会的”：看全文、删（`/api/bots/:id/skills`），或设成定时（一条发给它的定时，“照技能 X 做一遍”）。
- **Bot 之间（C5）**：`bot_message`（对象 Bot 的名字、话、是否等回话）。运行时把话投进对方的对话（`deliverBotMessage` → `messageBot`，来源 `bot`，对方在说话时排在这一轮之后），附件上下文里带来源标记 `[pisper:bot-message from=… chain=… hop=… reply=0|1]` 与一句“这条消息来自 Bot「X」”；转录投影据标记给这条消息加 `fromBot`，界面在气泡上写发话 Bot 的头像与名字（用户正文里写的标记不算）。对方这一轮说完（`afterBotTurn` 里的 `botTurnTrigger` + `forwardBotReplies`）：等回话的，回复交回去；它为这条链派的子 agent（`botAgentRequests`）做完后的汇报也交回去；交回去的话不再要求回话。限制（`BotLinkService`，只在内存里）：不能发给自己；同一条链最多 6 跳；同一对 Bot 每分钟最多 6 条；“收到”“谢谢”这类客气话不发；超限时链标为停下，两边时间线都写着“停在这里了”。发起方时间线里一条“在等 B 回话”（`GET /api/sessions/:id/agents` 的 `links`），对方回了就合上。权限不传递：对方的子 agent 只用对方自己的设定。执行节点上的 Bot（不是对话 + 子 agent 的）能收到话，但它的回复不会自动交回去。
- **主动（C6，默认关）**：Bot 设定多两项：`maxParallelAgents`（同时跑几个子 agent，1–6，默认 3）与 `proactive`（`enabled` 空闲时巡看、`everyHours` 1/3/6/12/24、`brief` 每日简报、`briefAt` 本地时间 HH:MM；只改其中一项时保留其余）。运行时初始化后起一个每分钟的计时器（`startBotProactiveTimer`，`dispose` 清掉；实验功能关着时什么都不做），对本机、开着、空闲（对话没在说话、手上没活、没有等批准）的 Bot 判断到没到点（`botProactiveDue`，上次时间存在 `<dataDir>/bots/<botId>/proactive.json`）。巡看：运行时直接派一个子 agent（标题“巡看”、快速档），工具只有 `read`/`grep`/`find`/`ls` 与 Bot 工具范围的交集，写操作根本不在它手上；不复盘成技能。做完交回对话的通知多一句“写成安静的建议，不开活”；这一轮里 `bot_job_start` 与 `bot_message` 直接拒绝，只能 `bot_remember`（来源 `proactive`）与 `bot_forget`，用户说“做吧”的那一轮才照常派活。简报：到了设定时间、当天还没写过时，往对话里说一条隐藏消息（`BOT_BRIEF_MARKER`，带过去 24 小时的活与今天的定时），对话写出简报，这一轮同样不开活。关掉巡看时正在跑的巡看也停下。
- **完成通知不丢**：对话这一轮的收尾等运行标记放下后再排（`finishBoardCard` 只记 `botTurnDue`，`streamPrompt` 收尾时排）；完成通知要 steer 进已经不在流式输出的一轮时改为放回待处理队列、收尾后唤醒补发；改设定时运行时受保护（`sessionRuntimeIsProtected`：在跑的子 agent、等唤醒的完成通知）就不释放。

## 架构：大脑、数据、执行环境分开

按 DeepSeek super agent 的思路把三样东西拆开：跑 Agent 循环的大脑、会话等数据、真正读写文件跑命令的执行环境。

| 层 | 内容 | 放在哪 |
| --- | --- | --- |
| 大脑 | Agent 循环、Bot 注册表、分派、审批、渠道连接 | Bot 所在的 Pisper Runtime：本机，或一台常开的服务器（npm 版） |
| 数据 | Bot 配置、它的会话 | 只在大脑所在 Runtime 的数据目录 |
| 执行环境 | 目录、命令、工具链 | 每个 Bot 自选：本机，或另一台运行 `pisper executor` 的电脑 |
| 凭据 | 仓库权限、内网账号 | 留在执行环境一侧，不进大脑 |

两种典型部署：全在本机（桌面关闭则 Bot 停）；大脑在服务器、执行在开发机（开发机离线时 Bot 的工具调用以“离线”失败，重新连上后照常）。

### 执行节点（B6）

`pisper executor` 是 npm 包里的无状态守护进程：**主动连回大脑**（HTTPS 长轮询，不开放端口），沿用远程访问的证书指纹配对；不保存会话、不持有模型密钥，只保存配对信息（0600），只在 `--root` 允许的目录里按真实路径做文件操作，`--no-shell` 时不执行任意命令。它不替谁跑 git。

Bot 的电脑是执行节点时，它的会话在大脑这边用一个占位目录（`<dataDir>/executor-sessions/<envId>`），会话元数据记 `remoteExecutor: { environmentId, cwd }`，会话的 read / edit / write / ls / find / bash 经 `NodeExecutor.operations()` 在那台电脑上执行（路径是那边的）；grep（依赖本机 ripgrep）、浏览器、计算机操作、MCP、插件、子代理、记忆等只在大脑本机可用的工具不提供，并由 `runToolBlocker` 在权限层拦下。审批不做本地改动预览。

## 概念模型

### Bot（持久化，`<dataDir>/pisper-bots.json`，最多 32 个，0600）

```ts
{
  id: string                     // bot_ + base64url
  name: string                   // 1–24 字符，全局唯一（不区分大小写），用于 @ 与 /<名字>；也是会话标题
  avatar: { character, color }   // 角色 + 颜色，前端生成会动的 SVG
  enabled: boolean               // false = 已暂停：渠道与定时不再交给它
  instructions: string           // ≤ 12_000 字符，进它会话的系统提示（botSystemPrompt）
  model: { tier } | { ref }      // 同 agent-profiles，打开会话与改设定时套到会话上
  thinking?: ThinkingLevel
  runMode: 'plan' | 'goal' | 'team'
  workspace: { environmentId: string, cwd: string }   // 'local' 或已配对节点；节点上的目录必须在它的 roots 内
  executionMode: ExecutionMode   // 上限，见「安全边界」
  tools: string[] | null         // null = 当前启用的全部工具
  channels: Array<{ platform, chatType, peerIds: string[] | null, senderIds: string[] }>
  defaultIn: string[]            // 作为默认 Bot 的聊天（platform:peerId）
  threadSessionId: string        // 它的长期会话
  createdAt, updatedAt: string
}
```

定时计划不放在 Bot 里：它们是 `ScheduleService` 中 `targetType = 'bot'` 的定时任务，Bot 设置页按 `botId` 显示。删除 Bot 时一并删除这些定时；它的会话留着，变成一个普通会话。

### Environment（`<dataDir>/pisper-environments.json`，最多 16 个）

`{ id, kind: 'local' | 'node', name, fingerprint, roots, capabilities: { shell }, lastSeenAt }`。一个执行环境可以被多个 Bot 共用；还有 Bot 在用时不能移除。

## Bot 与它的会话

- **建好就开会话**：`createBot` 之后 `ensureBotSession` 新建会话（标题 = 名字），并 `applyBotSettings`：电脑（执行节点时设 `remoteExecutor` 与占位目录）、目录、权限上限、模型。打开 Bot（`POST /api/bots/:id/session`）也走 `ensureBotSession`：会话文件不在了就补建。
- **设定在每一轮生效**：会话属于哪个 Bot 由 `botForSession` 判断（Bots 实验功能关着时一律不算）。`runSessionPrompt` 每一轮把工具范围设为 Bot 的 `tools`、上下文隔离（不读其他会话与全局记忆）——不管这句话来自应用、聊天还是定时任务；`createSessionRuntime` 把 `botSystemPrompt(bot)` 追加进系统提示。改设定时：目录、电脑、权限、模型立即套到会话上；指令与工具范围在会话空闲时释放运行时、下次重建后生效（正在跑的这一轮不打断）；改名时会话一起改名。
- **动态**：`botActivity` 直接读会话：在跑（`running`），在等批准或回答（`waiting: 'approval' | 'question'`，`needsYou`），否则空闲。侧栏头像随它动，看板卡片带 `bot: { id, name, avatar }`。
- **删除**：Bot 正在说话、或还有子 agent 在干活时拒绝（先停下）。它的记忆一并删掉，会话留着。

## 入口与分派

开关关闭、或消息没有点名任何 Bot 时，渠道行为与现在完全一致（直接和 Pisper 对话）。

分派是纯函数 `routeChannelMessage`（`bot-routing.mjs`）：

1. **点名**：消息以 `/<名字>` 开头，或 @Pisper 后紧跟 `<名字>`；私聊里也可以直接以“名字，”“名字：”或“名字 ”开头。名字不区分大小写，取最长匹配。
2. **默认 Bot**：没点名，但当前聊天在某个 Bot 的 `defaultIn` 里且 @ 了 Pisper（私聊不需要 @）。一个聊天最多一个默认 Bot（只能经接口设置）。
3. **校验**：Bot 已启用、`channels` 覆盖这个平台和聊天、发送者在 `senderIds` 里；私聊规则没写人时只有渠道创建者能叫它。不满足就在聊天里说明原因，只记脱敏摘要。
4. 都不满足 → 走原有直接对话。

点名后的话经 `messageBot` 发进 Bot 的会话（`promptFromChannel`）：

- Bot 空闲：这一轮开始，不另回“收到了”；说完后把它最后说的话（`botReplyText`，超过 3000 字截断并提示去 Pisper 里看全文）发回同一个聊天；出错时说“没做成：原因”。
- Bot 正在说话：回“正在忙别的事，等它说完再叫它，或者在 Pisper 里接着说”，不排队。v2 的对话只在回一句话的那几秒里“在说话”，派出去的活不占着它。
- 渠道里的附件暂不转给 Bot（回复里说明）。

这一轮是从哪个聊天叫起来的记在内存里（`botChatOrigins`，这一轮结束就清掉）：会话里出现审批或提问时，告诉那个聊天（普通审批附“/名字 批准 或 /名字 拒绝”；高风险只写“请在 Pisper 里看完再批准”；问题带出选项，回答在 Pisper 里）。在应用里说的话不往任何聊天发。聊天里整句是“/名字 批准 | 拒绝”时处理 Bot 会话里最早一条待审批；高风险在聊天里只能拒绝。其他命令：`/bots` 列出可叫的 Bot，`/<名字> 状态`（空闲 / 在忙 / 等批准 / 等回答），`/whoami`。

定时：`ScheduleService.executeBot` 调 `messageBot`（来源 `schedule`，无人值守，不提供 ask_user），用 Bot 自己的设定，不用定时任务的完全访问默认值；Bot 正在说话时这句话排在这一轮之后（`queueSessionMessage`，followUp）。要做的事存在 `botMessage`、`prompt` 留空。实验开关关闭时到点的 bot 定时记为失败并写明原因。

## HTTP 协议

路由在 `runtime/http/routes/bots.mjs`，两道门：能力 `bots` 与实验开关（关着时 409 `beta_feature_disabled`）。请求体一律按 `unknown` 校验。

| 方法 路径 | 用途 |
| --- | --- |
| `GET /api/bots` / `POST /api/bots` | 列表（含每个 Bot 的 `activity`）/ 新建（只给名字也行：目录默认用本机默认工作目录） |
| `GET /api/bots/:id` / `PATCH /api/bots/:id` / `DELETE /api/bots/:id` | 详情 / 修改（含暂停）/ 删除 |
| `POST /api/bots/:id/session` | 打开 Bot：返回 `{ sessionId }`（没有就新建） |
| `GET /api/environments` / `POST /api/environments/pairing` / `DELETE /api/environments/:id` | 执行环境列表（含在线状态）/ 生成配对码 / 移除 |

在应用里和 Bot 说话走普通会话的接口（`/api/chat`、排队与插话、审批与提问）。会话列表在 Bots 打开时给 Bot 的会话加 `botId` 与 `projectCwd`（Bot 的目录；节点上的 Bot 会话 cwd 只是占位目录），关着时这些会话不出现。

执行节点与大脑之间：

| 方法 路径 | 鉴权 | 用途 |
| --- | --- | --- |
| `POST /api/executor/pair` | 一次性配对码（10 分钟，猜错 20 次作废全部配对码） | `{ code, name, roots, capabilities }` → `{ environment, token }`；令牌只返回这一次，大脑只存 SHA-256 摘要 |
| `POST /api/executor/poll` | `Bearer <envId>.<令牌>` | 节点上报目录与能力，最多挂 25 秒领请求 `{ requests: [{ id, op, args }] }` |
| `POST /api/executor/results` | 同上 | `{ results: [{ id, ok, value \| error } \| { id, chunk }] }`，chunk 是 base64 的命令输出 |

这三个接口在远程监听上不要求设备令牌（`runtime/remote-auth.mjs` 白名单），由路由自己鉴权，仍受能力 `bots` 与实验开关约束。请求只有：`shell`（`--no-shell` 时拒绝）、`fs.read / write / access / stat / readdir / mkdir / glob`（按真实路径限制在 `--root` 里）、`cancel`；其余返回 `op_unsupported`。

所有响应必须是严格 JSON/UTF-8，`JSON.parse` 与 TUI 的 `serde_json` 都能解析。

## 客户端

| 位置 | 形态 |
| --- | --- |
| 侧栏 | 会话列表上方「Bots」分组，每个 Bot 一行：会动的头像（空闲 / 在说话 / 需要你）+ 名字，需要你时加琥珀点，动态写在悬停提示与读屏里。默认显示 3 个（需要你 → 在说话 → 其余），其余「显示全部」就地展开。点一下打开它的会话（`useOpenBot`：先 `POST /api/bots/:id/session` 再打开会话页）；正在看它的会话或设置页时这一行亮起。Bot 的会话不在侧栏“最近”里重复列出。开关关闭时整组不出现，也不加载这部分代码 |
| 会话页 | 就是普通会话页。认出 Bot 的会话（`session.botId`）时，标题前放它的头像、项目名后放「设置」按钮（`public-session.tsx`，按需加载） |
| 设置页 `/bots/:id` | 顶栏「和 它 聊天」回到会话；头像（「换个样子」才展开挑选）、名字、「它负责什么」；电脑、可以叫它的聊天、定时让它做、高级（接收聊天与定时的消息、权限上限、可用工具）都在折叠分区里，标题旁写当前值 |
| 电脑分区 | 选本机或已配对的执行节点（显示在线与否）与那台电脑上的目录；电脑列表（谁在用、移除前就地确认）；「添加电脑」就地显示一次性配对码与两条命令（含远程访问地址与证书指纹） |
| 新建 Bot | 对话框只要一个名字：头像随机，「它负责什么」可选，目录默认本机默认工作目录；建好直接打开它的会话 |
| 头像 | `avatar.ts` 生成 SVG + SMIL 动画，以 data URL 放进 `<img>`；系统要求减少动态时静止 |
| TUI | 没有 Bot 界面；Bot 的会话在会话列表里照常可见 |

界面约束沿用 AGENTS.md「组件与样式」。

## 安全边界

- **执行权限上限**：每个 Bot 的 `executionMode` 是上限，套在它的会话上；新建默认 `workspace-write`（界面上叫“危险操作才问”：命令守卫标出的危险命令、浏览器点击与填写、第三方插件才审批），已存下的 Bot 沿用自己的值。只要 `channels` 里含群聊，就不能选 `full-access`；定时找 Bot 不用定时任务的完全访问默认值。
- **不围工作目录**：Bot 的会话不受普通会话的工作目录围栏限制（`isUnfenced`），工作目录只是它开始干活的地方；目录外的读写照常按模式判定。执行节点上的文件操作仍限制在节点自己的 `--root` 里。
- **做不了才问**：系统提示要求 Bot 能自己做完的事直接做完，只有缺用户才知道的信息、要用户拍板或确实做不了时才问。
- **谁能叫 Bot**：群聊的 `senderIds` 不得为空；聊天里的“批准 / 拒绝”只接受白名单里的发送者。渠道连接级的 `accessMode = all` 不影响 Bot 的白名单。
- **指令与消息的边界**：Bot 指令由本机用户编写，进系统提示；渠道消息以普通用户消息进入，不拼进系统提示。Bot 不能经工具修改任何 Bot 的配置。
- **上下文隔离**：Bot 的会话每一轮都隔离上下文（`ISOLATED_CONTEXT_BLOCKED_TOOLS`），不读其他会话与全局记忆。
- **日志与展示**：发送者和聊天 id 掩码；被拒绝的点名只记脱敏摘要。
- **执行节点**：节点信任大脑——大脑一旦被攻破或被提示注入，连着的节点都会暴露。缓解：节点只在自己配置的 `roots` 内做文件操作（由节点按真实路径强制）；审批在大脑侧；`--no-shell` 关掉任意命令；建议用专门的系统用户或容器运行节点；配对要在节点那台机器上确认指纹。命令打印出的密钥会进上下文，这一风险与本地运行相同。

## 历史

- 2026-11-03：B1 注册表、B2 任务与 worktree 隔离、B3 渠道与定时、B4 应用内对话、B5 渠道回推、B6 执行节点上线（实验功能）。
- 2026-11-04：Bot 不再依赖 git；随后改为“Bot = 长期会话 + 设定”，删除任务服务、worktree、审阅与交付方式，渠道与定时直接对会话说话，Bot 页改成设置页。

## 参考

- [Exploring Super-agent Design（DeepSeek Harness，2026-11-01，GitHub 摘录）](https://github.com/yu-iskw/agent-works/issues/184)：计算、状态与执行环境分离；无状态的端点守护进程；间歇在线设备；安全模型的未解问题。
- [What Is Grok Bot（Layer3 Labs）](https://www.layer3labs.io/guides/what-is-grok-bot) 与 [How to use Grok Bot](https://www.layer3labs.io/guides/how-to-use-grok-bot)：每个 Bot 独立环境与登录、后台持续运行、自然语言规则、高风险操作需人工批准。
