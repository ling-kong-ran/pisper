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
  botActivityLine,
  botAvatarState,
  botToolsPreset,
  isActiveTaskStatus,
} from '../../src/features/bots/bot-presentation.ts'
import { parseBot, parseBotTask } from '../../src/features/bots/bots-api.ts'
import { parseBoardCard } from '../../src/features/board/board-api.ts'
import { buildBoardItems } from '../../src/features/board/board-model.ts'
import { buildSidebarModel } from '../../src/features/board/sidebar-model.ts'
import { agentSessionMethods } from '../runtime/agent-session-methods.mjs'

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
    delivery: 'email',
    workspace: { cwd: '/repo', isolation: 'weird' },
    activity: { taskId: 't1', status: 'review', title: '改按钮', changedFiles: 3, needsYou: true },
  })
  assert.deepEqual(bot.avatar, { character: 'blob', color: 'blue' })
  assert.equal(bot.executionMode, 'approval-required')
  assert.equal(bot.delivery, 'diff')
  assert.deepEqual(bot.workspace, { environmentId: 'local', cwd: '/repo', isolation: 'worktree' })
  assert.equal(bot.enabled, true)
  assert.equal(bot.activity?.changedFiles, 3)
  assert.throws(() => parseBot({ id: 'x' }))
  assert.throws(() => parseBot({ id: 'x', name: 'y', activity: { status: 'exploded' } }))
})

test('task parser keeps follow-up turns and reads old tasks without turns', () => {
  const task = parseBotTask({
    id: 'task_1',
    botId: 'bot_1',
    status: 'review',
    request: '第一句',
    turns: [
      { text: '第一句', at: '2026-11-03T00:00:00.000Z', reply: '改好了' },
      { text: '再改改', at: '2026-11-03T00:05:00.000Z' },
    ],
    result: { summary: 'ok', changedFiles: '2', branch: 'pisper/bot/abc' },
  })
  assert.equal(task.turns.length, 2)
  assert.equal(task.turns[0].reply, '改好了')
  assert.equal('reply' in task.turns[1], false)
  assert.equal(task.result?.changedFiles, 2)
  assert.equal(task.sessionId, null)

  const legacy = parseBotTask({
    id: 'task_0',
    botId: 'bot_1',
    status: 'done',
    request: '旧任务',
    createdAt: '2026-11-01T00:00:00.000Z',
  })
  assert.deepEqual(legacy.turns, [{ text: '旧任务', at: '2026-11-01T00:00:00.000Z' }])
  assert.throws(() => parseBotTask({ id: 'x', botId: 'y', status: 'nope' }))
})

test('task parser reads what a running task waits for and treats unknown risk as critical', () => {
  const base = { id: 't', botId: 'b', status: 'running', request: '清理' }
  assert.equal(parseBotTask(base).waiting, null)
  assert.equal(parseBotTask({ ...base, waiting: { approval: null, question: null } }).waiting, null)
  const waiting = parseBotTask({
    ...base,
    waiting: {
      approval: { id: 'a1', toolName: 'bash', argsPreview: 'rm -rf dist', severity: 'weird' },
      question: { id: 'q1', prompt: '删哪个？', options: ['dist', 3], count: 1 },
    },
  }).waiting
  assert.equal(waiting.approval.severity, 'critical')
  assert.equal(waiting.approval.argsTruncated, false)
  assert.deepEqual(waiting.question.options, ['dist'])
  assert.equal(waiting.question.multiSelect, false)
  assert.equal(
    parseBot({ id: 'b', name: 'n', activity: { status: 'running', waiting: 'nonsense' } }).activity
      .waiting,
    null,
  )
})

test('approving or answering in the chat goes through the session endpoints', async () => {
  const source = await readFile(join(root, 'src/features/bots/bots-api.ts'), 'utf8')
  assert.match(source, /\/api\/sessions\/\$\{encodeURIComponent\(sessionId\)\}\/approvals\//)
  assert.match(source, /\/api\/sessions\/\$\{encodeURIComponent\(sessionId\)\}\/questions\//)
  // 高风险必须在卡片里再确认一次；参数截断时不给“允许”。
  const card = await readFile(join(root, 'src/features/bots/BotWaitingCard.tsx'), 'utf8')
  assert.match(card, /critical \? setConfirming\(true\)/)
  assert.match(card, /!approval\.argsTruncated &&/)
})

test('sidebar line and avatar state follow the latest task', () => {
  const activity = (status, extra = {}) => ({
    taskId: 't',
    status,
    title: '修登录',
    changedFiles: 2,
    needsYou: false,
    updatedAt: '',
    ...extra,
  })
  assert.equal(botActivityLine(t, { enabled: false, activity: null }), 'bots:activity.paused')
  assert.equal(botActivityLine(t, { enabled: true, activity: null }), 'bots:activity.idle')
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('review', { needsYou: true }) }),
    'bots:activity.review {"count":2}',
  )
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity('running') }),
    'bots:activity.working {"title":"修登录"}',
  )
  assert.equal(
    botActivityLine(t, {
      enabled: true,
      activity: activity('running', { needsYou: true, waiting: 'approval' }),
    }),
    'bots:activity.approval',
  )
  assert.equal(
    botActivityLine(t, {
      enabled: true,
      activity: activity('running', { needsYou: true, waiting: 'question' }),
    }),
    'bots:activity.question',
  )
  assert.equal(
    botAvatarState({ activity: activity('running', { needsYou: true, waiting: 'approval' }) }),
    'need',
  )
  assert.equal(botAvatarState({ activity: activity('review', { needsYou: true }) }), 'need')
  assert.equal(botAvatarState({ activity: activity('preparing') }), 'running')
  assert.equal(botAvatarState({ activity: activity('done') }), 'idle')
  assert.equal(isActiveTaskStatus('queued'), true)
  assert.equal(isActiveTaskStatus('review'), false)
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
    botTasks: {
      activity: () => ({ branch: 'pisper/shen-cha-yuan/a1b2c3', changedFiles: 2 }),
    },
    isBetaFeatureEnabled: (id) => id === 'bots' && enabled,
  }
  assert.deepEqual(agentSessionMethods.botCardFor.call(runtime, 'thread-1'), {
    id: 'bot_1',
    name: '审查员',
    avatar: { character: 'owl', color: 'purple' },
    branch: 'pisper/shen-cha-yuan/a1b2c3',
    changedFiles: 2,
  })
  assert.equal(agentSessionMethods.botCardFor.call(runtime, 'other'), null)
  enabled = false
  assert.equal(agentSessionMethods.botCardFor.call(runtime, 'thread-1'), null)
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
      branch: 'pisper/x/y',
      changedFiles: 2,
    },
  })
  assert.equal(card.bot.name.includes('\u001b'), false)
  assert.equal(card.bot.character, 'owl')
  assert.equal(card.bot.changedFiles, 2)

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
