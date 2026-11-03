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
  botActivityLine,
  botAvatarState,
  isActiveTaskStatus,
} from '../../src/features/bots/bot-presentation.ts'
import { parseBot, parseBotTask } from '../../src/features/bots/bots-api.ts'

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
