import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { BETA_FEATURES } from '../../shared/beta-features.mjs'
import { BOT_AVATAR_CHARACTERS, BOT_AVATAR_COLORS } from '../services/bots/bot-model.mjs'
import { pageFromPath } from '../../src/app/routes.ts'
import {
  BOT_CHARACTERS,
  BOT_COLORS,
  botAvatarDataUrl,
  botAvatarSvg,
} from '../../src/features/bots/avatar.ts'
import {
  BOT_READ_ONLY_TOOLS,
  SIDEBAR_BOT_LIMIT,
  botActivityLine,
  botAvatarState,
  botDutyLine,
  botFolderName,
  botToolsPreset,
  sidebarBots,
} from '../../src/features/bots/bot-presentation.ts'
import { parseBot } from '../../src/features/bots/bots-api.ts'
import { parseBoardCard } from '../../src/features/board/board-api.ts'
import { buildBoardItems } from '../../src/features/board/board-model.ts'
import { buildSidebarModel } from '../../src/features/board/sidebar-model.ts'
import {
  groupSessionsByWorkspace,
  sessionWorkspaceKey,
} from '../../src/features/chat/session-workspaces.ts'
import { agentSessionMethods } from '../runtime/agent-session-methods.mjs'
import { buildBoardView } from '../services/session-board-view.mjs'

const root = fileURLToPath(new URL('../../', import.meta.url))
const t = (key, values) => (values ? `${key} ${JSON.stringify(values)}` : key)

test('Web avatars use the same characters and colors as the Runtime registry', () => {
  assert.deepEqual([...BOT_CHARACTERS], [...BOT_AVATAR_CHARACTERS])
  assert.deepEqual([...BOT_COLORS], [...BOT_AVATAR_COLORS])
})

test('every avatar renders as self-contained SVG and stays static when motion is off', () => {
  for (const character of BOT_CHARACTERS)
    for (const state of ['idle', 'running', 'need']) {
      const svg = botAvatarSvg({ character, color: 'teal' }, { state })
      assert.match(svg, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg"/)
      // 只用内联图形：不引用外部资源，也不带脚本。
      assert.doesNotMatch(svg, /<script|href=|url\(/i)
      assert.match(svg, /<animate/)
      assert.doesNotMatch(
        botAvatarSvg({ character, color: 'teal' }, { state, animate: false }),
        /<animate/,
      )
    }
  assert.match(botAvatarSvg({ character: 'fox', color: 'pink' }, { state: 'running' }), /rotate/)
  assert.match(
    botAvatarDataUrl({ character: 'owl', color: 'blue' }),
    /^data:image\/svg\+xml;charset=utf-8,%3Csvg/,
  )
})

test('bot parser falls back for unknown avatar and settings values', () => {
  const bot = parseBot({
    id: 'bot_1',
    name: '审查员',
    avatar: { character: 'dragon', color: 7 },
    executionMode: 'root',
    workspace: { cwd: '/repo' },
    threadSessionId: 'thread-1',
    activity: { status: 'running', waiting: 'approval' },
  })
  assert.deepEqual(bot.avatar, { character: 'blob', color: 'blue' })
  assert.equal(bot.executionMode, 'approval-required')
  assert.deepEqual(bot.workspace, { environmentId: 'local', cwd: '/repo' })
  assert.equal(bot.threadSessionId, 'thread-1')
  assert.equal(bot.enabled, true)
  // 模型与思考：缺失或不认识时按标准档、模型默认；指定模型必须是 provider/model。
  assert.deepEqual(bot.model, { tier: 'standard' })
  assert.equal(bot.thinking, null)
  const tuned = parseBot({
    id: 'b',
    name: 'n',
    model: { ref: 'luna/gpt-5.6-luna' },
    thinking: 'high',
  })
  assert.deepEqual(tuned.model, { ref: 'luna/gpt-5.6-luna' })
  assert.equal(tuned.thinking, 'high')
  assert.deepEqual(parseBot({ id: 'b', name: 'n', model: { ref: 'bare' } }).model, {
    tier: 'standard',
  })
  assert.deepEqual(bot.activity, {
    status: 'running',
    waiting: 'approval',
    needsYou: true,
    working: 0,
    current: '',
  })
  // 不认识的动态按空闲处理，不让整个列表因此读不出来。
  assert.deepEqual(parseBot({ id: 'b', name: 'n', activity: { status: 'exploded' } }).activity, {
    status: 'idle',
    waiting: null,
    needsYou: false,
    working: 0,
    current: '',
  })
  assert.throws(() => parseBot({ id: 'x' }))
})

test('a bot opens as its session; its settings live on their own page', async () => {
  // 打开 Bot 先问 Runtime 要会话 id（缺了由它补建），再打开普通会话页。
  const open = await readFile(join(root, 'src/features/bots/use-open-bot.ts'), 'utf8')
  assert.match(open, /const sessionId = await openBotSession\(bot\.id\)/)
  assert.match(open, /navigate\(sessionPath\(sessionId\)\)/)
  const sidebar = await readFile(join(root, 'src/features/bots/BotsSidebarSection.tsx'), 'utf8')
  assert.match(sidebar, /onClick=\{\(\) => void openBot\(bot\)\}/)
  // 会话页认出 Bot 的会话：按需加载头像与设置入口，不把 Bots 打进会话页的包。
  const header = await readFile(join(root, 'src/features/chat/SessionPageHeader.tsx'), 'utf8')
  assert.match(header, /lazy\(\(\) => import\('@\/features\/bots\/public-session'\)\)/)
  assert.match(header, /session\.botId && \(/)
  // 设置页只有设定，没有自己的对话框与任务列表。
  const page = await readFile(join(root, 'src/features/bots/BotsPage.tsx'), 'utf8')
  assert.doesNotMatch(page, /Textarea|sendBotMessage|BotConversation|useBotTasks/)
  for (const gone of ['BotConversation.tsx', 'BotWaitingCard.tsx', 'use-bot-tasks.ts'])
    await assert.rejects(readFile(join(root, 'src/features/bots', gone), 'utf8'), {
      code: 'ENOENT',
    })
})

test('sidebar line and avatar state follow the session', () => {
  const activity = (status, waiting = null) => ({ status, waiting, needsYou: Boolean(waiting) })
  assert.equal(botActivityLine(t, { enabled: false, activity: null }), 'bots:activity.paused')
  assert.equal(botActivityLine(t, { enabled: true, activity: null }), 'bots:activity.idle')
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('idle') }),
    'bots:activity.idle',
  )
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('running') }),
    'bots:activity.busy',
  )
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('running', 'approval') }),
    'bots:activity.approval',
  )
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('running', 'question') }),
    'bots:activity.question',
  )
  assert.equal(botAvatarState({ activity: activity('running', 'approval') }), 'need')
  assert.equal(botAvatarState({ activity: activity('running') }), 'running')
  assert.equal(botAvatarState({ activity: activity('idle') }), 'idle')
  assert.equal(botAvatarState({ activity: null }), 'idle')
})

test('Bots is a Labs feature with its own routes and lazily loaded sidebar group', async () => {
  const bots = BETA_FEATURES.find((feature) => feature.id === 'bots')
  assert.deepEqual(bots?.requires, ['bots'])
  assert.equal(pageFromPath('/bots'), 'bots')
  assert.equal(pageFromPath('/bots/bot_1'), 'bots')

  const app = await readFile(join(root, 'src/App.tsx'), 'utf8')
  assert.match(app, /import\('@\/features\/bots\/public-sidebar'\)/)
  assert.doesNotMatch(app, /from '@\/features\/bots/)
  const labs = await readFile(join(root, 'src/features/labs/LabsSettings.tsx'), 'utf8')
  assert.match(labs, /t\('labs:feature\.bots\.title'\)/)
  // 侧栏与 Bot 页只取开关 Hook，不连带加载实验功能设置页。
  for (const file of ['BotsPage.tsx', 'BotsSidebarSection.tsx'])
    assert.match(
      await readFile(join(root, 'src/features/bots', file), 'utf8'),
      /from '@\/features\/labs\/public-flags'/,
    )
})

test('tool scope presets map to the list the Runtime enforces', () => {
  assert.equal(botToolsPreset(null), 'all')
  assert.equal(botToolsPreset([...BOT_READ_ONLY_TOOLS].reverse()), 'read-only')
  assert.equal(botToolsPreset(['read']), 'custom')
  assert.equal(botToolsPreset([]), 'custom')
  // 只读名单里不能有会改文件或跑命令的工具。
  for (const name of ['bash', 'edit', 'write']) assert.ok(!BOT_READ_ONLY_TOOLS.includes(name))
  assert.equal(parseBot({ id: 'b', name: 'n' }).tools, null)
  assert.deepEqual(parseBot({ id: 'b', name: 'n', tools: ['read', 7] }).tools, ['read'])
})

test('a bot thread session carries its bot onto board cards only while Bots is on', () => {
  const bots = [
    {
      id: 'bot_1',
      name: '审查员',
      avatar: { character: 'owl', color: 'purple' },
      threadSessionId: 'thread-1',
    },
  ]
  let enabled = true
  const runtime = {
    bots: { list: () => bots },
    isBetaFeatureEnabled: (id) => id === 'bots' && enabled,
  }
  assert.deepEqual(agentSessionMethods.botCardFor.call(runtime, 'thread-1'), {
    id: 'bot_1',
    name: '审查员',
    avatar: { character: 'owl', color: 'purple' },
  })
  assert.equal(agentSessionMethods.botCardFor.call(runtime, 'other'), null)
  enabled = false
  assert.equal(agentSessionMethods.botCardFor.call(runtime, 'thread-1'), null)
})

test('bot sessions carry their bot and folder, and vanish while Bots is off', async () => {
  // Bot 在执行节点上时会话 cwd 只是大脑这边的占位目录：列表带上 Bot 的目录（归组与看板筛选按它）
  // 与 botId（侧栏“最近”不重复列出，会话页头换成 Bot）。实验开关关掉时 Bot 的会话不出现。
  const placeholder = '/data/executor-sessions/env_testmachine1'
  let enabled = true
  const runtime = {
    sessionLifecycle: {
      listSessions: async () => [
        { id: 'thread-1', cwd: placeholder },
        { id: 'plain', cwd: '/repo' },
      ],
    },
    bots: {
      list: () => [{ id: 'bot_1', threadSessionId: 'thread-1', workspace: { cwd: '/repo' } }],
    },
    isBetaFeatureEnabled: (id) => id === 'bots' && enabled,
  }
  runtime.botSessionProjects = agentSessionMethods.botSessionProjects
  const sessions = await agentSessionMethods.listSessions.call(runtime)
  assert.deepEqual(sessions, [
    { id: 'thread-1', cwd: placeholder, botId: 'bot_1', projectCwd: '/repo' },
    { id: 'plain', cwd: '/repo' },
  ])
  const groups = groupSessionsByWorkspace(sessions)
  assert.equal(groups.length, 1)
  assert.equal(groups[0].cwd, '/repo')
  assert.equal(sessionWorkspaceKey(sessions[0]), sessionWorkspaceKey(sessions[1]))
  const items = sessions.map((session) => ({ ...session, card: { boardStatus: 'done' } }))
  assert.equal(
    buildBoardView(items, { cwds: ['/repo'], lanes: [], limit: 10, cursor: '' }).total,
    2,
  )
  enabled = false
  assert.deepEqual(await agentSessionMethods.listSessions.call(runtime), [
    { id: 'plain', cwd: '/repo' },
  ])
})

test('board cards parse the bot defensively and keep plain sessions unchanged', () => {
  assert.equal('bot' in parseBoardCard({ boardStatus: 'running' }), false)
  assert.equal('bot' in parseBoardCard({ bot: { id: 'b' } }), false)
  const card = parseBoardCard({
    boardStatus: 'attention',
    bot: {
      id: 'bot_1',
      name: '审查员\u001b[31m',
      avatar: { character: 'owl', color: 'purple' },
    },
  })
  assert.equal(card.bot.name.includes('\u001b'), false)
  assert.equal(card.bot.character, 'owl')

  const now = Date.parse('2026-11-03T10:00:00.000Z')
  const items = buildBoardItems(
    [],
    {
      live: {},
      localRuns: {},
      runningSince: {},
      patches: {},
      pending: {},
      feed: {
        ready: true,
        entries: {
          'thread-1': {
            session: { id: 'thread-1', name: '审查员 · 修按钮', cwd: '/w', streaming: true },
            card: parseBoardCard({ boardStatus: 'running', bot: { id: 'bot_1', name: '审查员' } }),
            revision: 1,
          },
          plain: {
            session: { id: 'plain', name: '普通会话', cwd: '/w', streaming: true },
            card: parseBoardCard({ boardStatus: 'running' }),
            revision: 1,
          },
        },
      },
    },
    { now, timeZoneOffsetMinutes: 0 },
  )
  const byId = Object.fromEntries(items.map((item) => [item.id, item]))
  assert.equal(byId['thread-1'].bot.name, '审查员')
  assert.equal('bot' in byId.plain, false)
  const rows = buildSidebarModel(items, { now }).groups.flatMap((group) => group.rows)
  assert.equal(rows.find((row) => row.id === 'thread-1').bot.id, 'bot_1')
  assert.equal('bot' in rows.find((row) => row.id === 'plain'), false)
})

test('computer helpers name machines and build the executor commands', async () => {
  const { environmentLabel, executorCommands } =
    await import('../../src/features/bots/bot-environments.ts')
  const { parseEnvironment } = await import('../../src/features/bots/bots-api.ts')
  assert.equal(environmentLabel(t, undefined), 'bots:computer.local')
  const node = parseEnvironment({
    id: 'env_testmachine1',
    kind: 'node',
    name: '',
    online: true,
    roots: ['/work', 3],
    capabilities: { shell: true },
    usedBy: [{ id: 'b', name: '审查员' }, { id: 1 }],
  })
  assert.equal(environmentLabel(t, node), 'bots:computer.unnamed')
  assert.deepEqual(node.roots, ['/work'])
  assert.deepEqual(node.usedBy, [{ id: 'b', name: '审查员' }])
  const placeholders = { brain: 'BRAIN', fingerprint: 'FP', root: '~/code' }
  const [pair, run] = executorCommands(
    {
      code: 'ABCD-EF23',
      remote: { enabled: true, endpoints: ['https://10.0.0.2:5174'], fingerprint: 'SHA256:AA' },
    },
    placeholders,
  )
  assert.equal(
    pair,
    'npx pisper executor pair --brain https://10.0.0.2:5174 --fingerprint SHA256:AA --code ABCD-EF23 --root ~/code',
  )
  assert.equal(run, 'npx pisper executor')
  assert.match(
    executorCommands(
      { code: 'X', remote: { enabled: false, endpoints: [], fingerprint: null } },
      placeholders,
    )[0],
    /--brain BRAIN --fingerprint FP/,
  )
})

test('a bot needs only a name: the folder defaults and settings stay out of the first view', async () => {
  const { AgentRuntimeFacade } = await import('../runtime/agent-runtime-facade.mjs')
  const created = []
  const runtime = {
    cwd: '/default/workspace',
    bots: {
      create: async (input) => (created.push(input), { id: input.name, ...input }),
      get: (id) => ({ id }),
    },
    // 建好就开它的会话（见 bot-sessions.test.mjs）：这里只看目录默认值。
    ensureBotSession: async () => 'thread',
  }
  await AgentRuntimeFacade.prototype.createBot.call(runtime, { name: '助手' })
  await AgentRuntimeFacade.prototype.createBot.call(runtime, {
    name: '远程',
    workspace: { environmentId: 'env_testmachine1', cwd: '' },
  })
  await AgentRuntimeFacade.prototype.createBot.call(runtime, {
    name: '指定',
    workspace: { cwd: '/repo' },
  })
  assert.equal(created[0].workspace.cwd, '/default/workspace')
  // 执行节点上的目录没有默认值，交给校验去拒绝，不悄悄用本机路径。
  assert.equal(created[1].workspace.cwd, '')
  assert.equal(created[2].workspace.cwd, '/repo')

  const dialog = await readFile(join(root, 'src/features/bots/NewBotDialog.tsx'), 'utf8')
  assert.match(dialog, /disabled=\{saving \|\| !name\.trim\(\)\}/)
  const details = await readFile(join(root, 'src/features/bots/BotDetails.tsx'), 'utf8')
  for (const section of ['computer', 'chats', 'schedules', 'advanced'])
    assert.match(details, new RegExp(`<AccordionItem value="${section}">`))
})

test('the sidebar keeps the bots that matter on top and folds the rest', () => {
  // 需要你 → 正在说话 → 其余保持原顺序。只显示前几个，当前打开的那个总留着。
  const activity = (status, waiting = null) => ({ status, waiting, needsYou: Boolean(waiting) })
  const bots = [
    { id: 'idle', activity: null },
    { id: 'first', activity: activity('idle') },
    { id: 'running', activity: activity('running') },
    { id: 'second', activity: activity('idle') },
    { id: 'waiting', activity: activity('running', 'approval') },
  ]
  const ids = (result) => result.shown.map((bot) => bot.id)
  const all = sidebarBots(bots, { expanded: true })
  assert.deepEqual(ids(all), ['waiting', 'running', 'idle', 'first', 'second'])
  assert.equal(all.hidden, 0)
  const folded = sidebarBots(bots, { expanded: false })
  assert.equal(folded.shown.length, SIDEBAR_BOT_LIMIT)
  assert.equal(folded.hidden, bots.length - SIDEBAR_BOT_LIMIT)
  const withCurrent = sidebarBots(bots, { expanded: false, currentId: 'second' })
  assert.deepEqual(ids(withCurrent), ['waiting', 'running', 'idle', 'second'])
  assert.equal(withCurrent.hidden, 1)
  // 不多于上限时全部显示，没有“显示全部”。
  assert.equal(sidebarBots(bots.slice(0, SIDEBAR_BOT_LIMIT), { expanded: false }).hidden, 0)
})

test('the Bots settings page leaves the Windows window controls alone', async () => {
  // Windows 自绘标题栏的窗口按钮绝对定位在右上 144×48：设置页顶栏与会话顶栏一样让出这段。
  const page = await readFile(join(root, 'src/features/bots/BotsPage.tsx'), 'utf8')
  assert.match(
    page,
    /<header\s+data-window-drag-region\s+className="flex h-\[52px\][^"]*\[\[data-custom-titlebar\]_&\]:pr-\[144px\]"/,
  )
})

// Bot 的会话：回复信息行换成 Bot 的头像与名字（Bots 关着时退回 Pisper 抬头），模型与思考在设置页里定。
test('bot sessions show the bot in reply heads and keep model settings on the settings page', async () => {
  const read = (path) => readFile(join(root, path), 'utf8')
  const [message, focus, details] = await Promise.all([
    read('src/features/chat/ChatMessage.tsx'),
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/bots/BotDetails.tsx'),
  ])
  assert.match(
    message,
    /part="mark"\s+animate=\{latest\}\s+fallback=\{<BrandLogo size=\{22\} \/>\}/,
  )
  assert.match(message, /part="name"/)
  assert.match(
    focus,
    /pageChrome \? \{ model: modelLabel, \.\.\.\(botId \? \{ botId \} : \{\}\) \} : null/,
  )
  assert.match(details, /<BotModelSection bot=\{bot\}/)
})

// Bot 的招呼：只由设定算出来，留在对话最前面；三个动作都对应已有设定，不调用模型。
test('bot greeting derives from settings and stays at the start of the conversation', async () => {
  assert.equal(botFolderName('/home/me/acme-web/'), 'acme-web')
  assert.equal(botFolderName('C:\\work\\acme'), 'acme')
  assert.equal(botFolderName('/'), '/')
  assert.equal(botDutyLine('  盯测试\n\n有问题  告诉我 '), '盯测试 有问题 告诉我')
  assert.equal(botDutyLine('字'.repeat(80)), '字'.repeat(80))
  assert.equal(botDutyLine('字'.repeat(81)), `${'字'.repeat(80)}…`)

  const read = (path) => readFile(join(root, path), 'utf8')
  const [greeting, transcript, focus, page, zh, en] = await Promise.all([
    read('src/features/bots/BotGreeting.tsx'),
    read('src/features/chat/FocusTranscript.tsx'),
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/bots/BotsPage.tsx'),
    read('src/locales/zh-CN/bots.json'),
    read('src/locales/en-US/bots.json'),
  ])
  // 实验功能关着或 Bot 已删除时退回普通空状态。
  assert.match(greeting, /if \(!enabled\) return fallback/)
  assert.match(greeting, /if \(!bot\) return loaded \? fallback : null/)
  // 只在本机才给简报与目录选择器：执行节点上的 Bot 用不了。
  assert.match(greeting, /const local = bot\.workspace\.environmentId === 'local'/)
  // 不经 Runtime 对话接口，也不进转录。
  assert.doesNotMatch(greeting, /sendMessage|messageBot|\/api\/chat/)
  // 空对话显示完整招呼；有消息时只在最前面显示收起版，还有更早消息时不显示。
  assert.match(transcript, /<BotGreeting botId=\{botId\} fallback=\{emptyConversation\} \/>/)
  assert.match(
    transcript,
    /messages\.length > 0 &&\s+!hasOlder &&\s+!collapsed && \(\s+<Suspense fallback=\{null\}>\s+<BotGreeting botId=\{botId\} collapsed \/>/,
  )
  assert.match(focus, /botId=\{botId\}\s+key="transcript"/)
  // 只认聊天分区，地址里的其他值不进 Accordion。
  assert.match(page, /searchParams\.get\('section'\) === 'chats' \? 'chats' : undefined/)
  const keys = (json) =>
    Object.keys(JSON.parse(json))
      .filter((key) => key.startsWith('greeting.'))
      .sort()
  assert.ok(keys(zh).length > 0)
  assert.deepEqual(keys(zh), keys(en))
})
