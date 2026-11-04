# Bot 服务

状态（2026-11-04）：**Bot 就是一个长期会话加上它的设定**。打开 Bot 就是打开这个会话（普通会话页：实时回复、插话、附件、审批与提问都是现成的）；渠道里叫它、定时让它做，都是往这个会话里说一句话。没有任务、队列、独立分支或审阅这一层，Bot 不依赖 git——真要开发是派给代理去做，不是 Bot 自己在分支上改。

主要文件：`bot-model.mjs`（校验与归一、系统提示、消息校验）、`bot-service.mjs`（注册表）、`bot-routing.mjs`（渠道分派）、`bot-notifications.mjs`（发回聊天的文字）、`environment-service.mjs` / `executor-hub.mjs` / `executor.mjs`（执行节点）、`runtime/http/routes/{bots,executors}.mjs`、`runtime/runtime/agent-runtime-facade.mjs` 里的 Bot 接线（`ensureBotSession`、`applyBotSettings`、`messageBot`、`botActivity`、`botForSession`）、`runtime/runtime/executor-tools.mjs`、`packages/pisper/lib/executor-node.mjs`；测试 `runtime/tests/bot-*.test.mjs`、`bots-frontend.test.mjs`。以代码、测试为准并同步修订本文。上线方式：实验功能 `bots`，默认关闭（见 `src/features/labs/README.md`）。

## 目标与非目标

参考 Grok Bot / Dots：用户可以有多个常驻的 Bot，每个有名字、头像、它负责什么（指令）、在哪台电脑的哪个目录里干活、权限上限、可用工具，以及谁能在聊天里叫它、什么时候定时找它。和 Bot 说话就像和同事聊天：一个长期对话，前文都算数，它说话时实时看得到，可以随时插话。

非目标（以后再说）：每个 Bot 独立的记忆范围与浏览器登录态；每个 Bot 绑定自己的渠道机器人账号；GitHub webhook 触发；Bot 主动找你（除定时外）。

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
- **删除**：Bot 正在说话时拒绝（先停下这一轮）。

## 入口与分派

开关关闭、或消息没有点名任何 Bot 时，渠道行为与现在完全一致（直接和 Pisper 对话）。

分派是纯函数 `routeChannelMessage`（`bot-routing.mjs`）：

1. **点名**：消息以 `/<名字>` 开头，或 @Pisper 后紧跟 `<名字>`；私聊里也可以直接以“名字，”“名字：”或“名字 ”开头。名字不区分大小写，取最长匹配。
2. **默认 Bot**：没点名，但当前聊天在某个 Bot 的 `defaultIn` 里且 @ 了 Pisper（私聊不需要 @）。一个聊天最多一个默认 Bot（只能经接口设置）。
3. **校验**：Bot 已启用、`channels` 覆盖这个平台和聊天、发送者在 `senderIds` 里；私聊规则没写人时只有渠道创建者能叫它。不满足就在聊天里说明原因，只记脱敏摘要。
4. 都不满足 → 走原有直接对话。

点名后的话经 `messageBot` 发进 Bot 的会话（`promptFromChannel`）：

- Bot 空闲：这一轮开始，不另回“收到了”；说完后把它最后说的话（`botReplyText`，超过 3000 字截断并提示去 Pisper 里看全文）发回同一个聊天；出错时说“没做成：原因”。
- Bot 正在说话：回“正在忙别的事，等它说完再叫它，或者在 Pisper 里接着说”，不排队。
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
