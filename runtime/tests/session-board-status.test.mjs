import assert from 'node:assert/strict'
import test from 'node:test'
import {
  APPROVAL_SEVERITIES,
  BOARD_LANES,
  BOARD_STATUSES,
  COMMAND_GUARD_REASON_PREFIX,
  approvalSeverity,
  compareBoardItems,
  deriveBoardStatus,
  isBoardLane,
  isBoardStatus,
  isWithinDoneWindow,
  sortBoardItems,
} from '../../shared/session-board-status.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'

// 固定在东八区 2026-10-29 10:00；显式偏移让结果与运行机器的时区无关。
const OFFSET = 480
const NOW = Date.parse('2026-10-29T02:00:00.000Z')
const options = { now: NOW, timeZoneOffsetMinutes: OFFSET }
const at = (local) => new Date(Date.parse(`${local}+08:00`)).toISOString()
const derive = (session, extra = {}) => deriveBoardStatus(session, { ...options, ...extra })

test('board lane enums stay closed and tolerate unknown values', () => {
  assert.deepEqual([...BOARD_LANES], ['attention', 'running', 'queued', 'done'])
  assert.deepEqual(
    [...BOARD_STATUSES],
    ['approval', 'question', 'failed', 'running', 'queued', 'done', 'stopped'],
  )
  assert.deepEqual([...APPROVAL_SEVERITIES], ['standard', 'critical'])
  assert.equal(isBoardLane('attention'), true)
  assert.equal(isBoardLane('idle'), false)
  assert.equal(isBoardStatus('stopped'), true)
  assert.equal(isBoardStatus(42), false)
  for (const value of [null, undefined, 1, 'x', [], { attentionReason: 7, streaming: 'yes' }]) {
    const result = derive(value)
    assert.equal(result.lane, null)
    assert.equal(result.status, 'done')
  }
})

test('attention lane holds approvals and unread failures, archived sessions never appear', () => {
  const approval = derive({
    id: 'a',
    needsAttention: true,
    attentionReason: 'approval',
    streaming: true,
    modified: at('2026-10-20T08:00:00'),
  })
  assert.deepEqual(approval, {
    lane: 'attention',
    status: 'approval',
    handled: false,
    since: Date.parse(at('2026-10-20T08:00:00')),
  })
  // 审批详情带 createdAt 时以最早的一条作为等待起点。
  const withApprovals = derive({
    attentionReason: 'approval',
    approvals: [
      { id: '2', createdAt: at('2026-10-29T09:30:00') },
      { id: '1', createdAt: at('2026-10-29T09:10:00') },
      { id: '3', createdAt: 'not a date' },
    ],
    modified: at('2026-10-29T09:50:00'),
  })
  assert.equal(withApprovals.since, Date.parse(at('2026-10-29T09:10:00')))

  const failure = derive({
    needsAttention: true,
    attentionReason: 'failure',
    unread: true,
    lastCompletedAt: at('2026-10-27T23:00:00'),
  })
  assert.deepEqual(failure, {
    lane: 'attention',
    status: 'failed',
    handled: false,
    since: Date.parse(at('2026-10-27T23:00:00')),
  })
  // 字段缺失时不能把失败当作已处理而悄悄移出“需要你”。
  assert.equal(derive({ attentionReason: 'failure' }).lane, 'attention')

  for (const reason of ['approval', 'failure']) {
    assert.equal(
      derive({ archived: true, pinned: true, attentionReason: reason, unread: true }).lane,
      null,
    )
  }
  assert.equal(derive({ archived: true, streaming: true }).lane, null)
  assert.equal(derive({ archived: true, streaming: true }).status, 'running')
})

test('needsAttention false wins over a stale reason and unknown reasons fall through', () => {
  assert.equal(derive({ needsAttention: false, attentionReason: 'approval' }).lane, null)
  const unknown = derive({ needsAttention: true, attentionReason: 'teleport', streaming: true })
  assert.deepEqual([unknown.lane, unknown.status], ['running', 'running'])
  const idle = derive({ needsAttention: true, attentionReason: 'teleport' })
  assert.deepEqual([idle.lane, idle.status], [null, 'done'])
})

test('handled failures leave attention and stay in the done window as failed but viewed', () => {
  const today = derive({
    attentionReason: 'failure',
    needsAttention: true,
    unread: false,
    lastCompletedAt: at('2026-10-29T08:00:00'),
  })
  assert.deepEqual(today, {
    lane: 'done',
    status: 'failed',
    handled: true,
    since: Date.parse(at('2026-10-29T08:00:00')),
  })
  const old = derive({
    attentionReason: 'failure',
    unread: false,
    lastCompletedAt: at('2026-10-20T08:00:00'),
  })
  assert.deepEqual([old.lane, old.status, old.handled], [null, 'failed', true])
  assert.equal(
    derive({
      attentionReason: 'failure',
      unread: false,
      pinned: true,
      modified: at('2026-01-01T00:00:00'),
    }).lane,
    'done',
  )
})

test('running wins over a failure that is being retried, but never over a pending approval', () => {
  const retrying = derive({
    attentionReason: 'failure',
    needsAttention: true,
    unread: true,
    streaming: true,
    startedAt: at('2026-10-29T09:59:00'),
  })
  assert.deepEqual(retrying, {
    lane: 'running',
    status: 'running',
    handled: false,
    since: Date.parse(at('2026-10-29T09:59:00')),
  })
  assert.equal(derive({ streaming: true }).since, null)
  assert.equal(
    derive({ streaming: true, card: { startedAt: at('2026-10-29T09:00:00') } }).since,
    Date.parse(at('2026-10-29T09:00:00')),
  )
  assert.equal(derive({ streaming: true, attentionReason: 'approval' }).status, 'approval')
})

test('question and queued states require their runtime capabilities', () => {
  const question = {
    needsAttention: true,
    attentionReason: 'question',
    streaming: true,
    card: { question: { id: 'q', createdAt: at('2026-10-29T09:40:00') }, queue: { position: 1 } },
  }
  assert.deepEqual(
    [derive(question).lane, derive(question).status],
    ['running', 'running'],
    'without askUser a question must never surface',
  )
  const answered = derive(question, { capabilities: { askUser: true } })
  assert.deepEqual(
    [answered.lane, answered.status, answered.since],
    ['attention', 'question', Date.parse(at('2026-10-29T09:40:00'))],
  )
  assert.equal(derive({ attentionReason: 'question' }).lane, null)

  const queued = {
    queue: { position: 2, startAfter: 'other' },
    modified: at('2026-10-29T09:00:00'),
  }
  assert.deepEqual([derive(queued).lane, derive(queued).status], ['done', 'done'])
  const withQueue = derive(queued, { capabilities: { sessionQueue: true } })
  assert.deepEqual([withQueue.lane, withQueue.status], ['queued', 'queued'])
  assert.equal(derive({ queue: {} }, { capabilities: { sessionQueue: true } }).lane, null)
  assert.equal(derive({ queue: 'soon' }, { capabilities: { sessionQueue: true } }).lane, null)
})

test('done lane keeps today completions, today activity and pinned sessions only', () => {
  const completedToday = derive({ lastCompletedAt: at('2026-10-29T00:10:00') })
  assert.deepEqual(completedToday, {
    lane: 'done',
    status: 'done',
    handled: false,
    since: Date.parse(at('2026-10-29T00:10:00')),
  })
  // “今天”按本地日期：东八区午夜前一分钟属于昨天；东八区 10-29 07:00 在 UTC 仍是 10-28。
  assert.equal(derive({ lastCompletedAt: at('2026-10-28T23:59:00') }).lane, null)
  assert.equal(derive({ lastCompletedAt: at('2026-10-29T07:00:00') }).lane, 'done')
  assert.equal(
    derive({ lastCompletedAt: at('2026-10-29T07:00:00') }, { timeZoneOffsetMinutes: 0 }).lane,
    null,
  )
  assert.equal(derive({ modified: at('2026-10-29T08:00:00'), messageCount: 4 }).lane, 'done')
  assert.equal(derive({ modified: at('2026-10-29T08:00:00') }).lane, 'done')
  // 从未发消息的空会话只是“创建过”，不算今天有活动。
  assert.equal(derive({ modified: at('2026-10-29T08:00:00'), messageCount: 0 }).lane, null)
  assert.equal(
    derive({ modified: at('2026-10-29T08:00:00'), messageCount: 0, pinned: true }).lane,
    'done',
  )
  assert.equal(derive({ modified: at('2026-10-27T08:00:00'), messageCount: 3 }).lane, null)
  assert.equal(derive({ pinned: true, modified: at('2025-01-01T08:00:00') }).lane, 'done')
  // Runtime 时钟略快于客户端时按最近处理，不让刚完成的会话消失。
  assert.equal(derive({ lastCompletedAt: at('2026-10-29T10:05:00') }).lane, 'done')

  const lastWeek = { lastCompletedAt: at('2026-10-23T09:00:00') }
  assert.equal(derive(lastWeek).lane, null)
  assert.equal(derive(lastWeek, { doneWindow: '7d' }).lane, 'done')
  assert.equal(
    derive({ lastCompletedAt: at('2026-10-22T23:00:00') }, { doneWindow: '7d' }).lane,
    null,
  )
  assert.equal(derive(lastWeek, { doneWindow: 'forever' }).lane, null)
  // 当前没有可区分中止的字段，stopped 不会被推导出来。
  assert.notEqual(
    derive({ lastCompletedAt: at('2026-10-29T09:00:00'), stopped: true }).status,
    'stopped',
  )
})

test('done window helper accepts dates, numbers and strings and rejects garbage', () => {
  assert.equal(isWithinDoneWindow(new Date(NOW), options), true)
  assert.equal(isWithinDoneWindow(NOW - 60_000, options), true)
  assert.equal(isWithinDoneWindow('yesterday', options), false)
  assert.equal(isWithinDoneWindow(null, options), false)
  assert.equal(isWithinDoneWindow(NOW, { now: 'nope' }), false)
  // 未传偏移时按系统时区计算，“现在”总是今天。
  assert.equal(isWithinDoneWindow(Date.now(), { now: Date.now() }), true)
})

test('approval severity uses the runtime field, then the command guard reason prefix', () => {
  assert.equal(approvalSeverity({ severity: 'critical', reason: 'anything' }), 'critical')
  assert.equal(
    approvalSeverity({ severity: 'standard', reason: `${COMMAND_GUARD_REASON_PREFIX}：强推` }),
    'standard',
  )
  assert.equal(
    approvalSeverity({ severity: 'extreme', reason: '命令守卫拦截：递归删除。' }),
    'critical',
  )
  assert.equal(
    approvalSeverity({ reason: '  命令守卫拦截：改写远端分支。请确认后执行。' }),
    'critical',
  )
  assert.equal(approvalSeverity({ risk: 'high', reason: 'bash 将以当前用户权限运行' }), 'standard')
  for (const value of [null, undefined, 'critical', [], { reason: 42 }])
    assert.equal(approvalSeverity(value), 'standard')
})

test('the degraded severity rule matches the reasons the permission service really writes', () => {
  const guarded = permissionRequirement({
    mode: 'auto',
    executionMode: 'approval-required',
    cwd: process.cwd(),
    toolName: 'bash',
    args: { command: 'git reset --hard HEAD~1' },
  })
  assert.ok(guarded && !guarded.block, 'warn-level guard hits ask instead of hard blocking')
  assert.equal(guarded.risk, 'high')
  assert.equal(approvalSeverity(guarded), 'critical')

  const ordinary = permissionRequirement({
    mode: 'ask',
    executionMode: 'approval-required',
    cwd: process.cwd(),
    toolName: 'bash',
    args: { command: 'npm test' },
  })
  // 几乎所有审批都是 risk: 'high'，因此只能按命令守卫区分，普通命令保持一步批准。
  assert.equal(ordinary.risk, 'high')
  assert.equal(approvalSeverity(ordinary), 'standard')
  const edit = permissionRequirement({
    mode: 'ask',
    executionMode: 'approval-required',
    cwd: process.cwd(),
    toolName: 'edit',
    args: { path: 'src/index.ts' },
  })
  assert.equal(approvalSeverity(edit), 'standard')
})

test('degraded severity marks the non-bash requests the spec lists as critical', () => {
  // 规格 3.5：安装全局插件、全局写入、可能提交表单的浏览器交互都必须走确认层，不能一键盲批。
  assert.equal(approvalSeverity({ toolName: 'plugin_create', args: { name: 'x' } }), 'critical')
  assert.equal(
    approvalSeverity({ toolName: 'skill_create', args: { scope: 'global', name: 'x' } }),
    'critical',
  )
  assert.equal(
    approvalSeverity({ toolName: 'skill_create', args: { scope: 'project' } }),
    'standard',
  )
  for (const action of ['click', 'type'])
    assert.equal(
      approvalSeverity({ toolName: 'browser_automation', args: { action } }),
      'critical',
      action,
    )
  assert.equal(
    approvalSeverity({ toolName: 'browser_automation', args: { action: 'screenshot' } }),
    'standard',
  )
  // Runtime 明确给出的 severity 仍然优先。
  assert.equal(
    approvalSeverity({ toolName: 'plugin_create', severity: 'standard', args: {} }),
    'standard',
  )
  // 与权限服务真实写出的请求对得上：plugin_create 与浏览器点击都会进入审批。
  const plugin = permissionRequirement({
    mode: 'ask',
    executionMode: 'approval-required',
    cwd: process.cwd(),
    toolName: 'plugin_create',
    args: { name: 'demo' },
  })
  assert.ok(plugin && !plugin.block)
  assert.equal(approvalSeverity({ ...plugin, toolName: 'plugin_create', args: {} }), 'critical')
  const click = permissionRequirement({
    mode: 'auto',
    executionMode: 'approval-required',
    cwd: process.cwd(),
    toolName: 'browser_automation',
    args: { action: 'click', ref: 'e1' },
  })
  assert.ok(click && !click.block)
  assert.equal(
    approvalSeverity({ ...click, toolName: 'browser_automation', args: { action: 'click' } }),
    'critical',
  )
})

test('lane comparator follows the fixed per-lane ordering', () => {
  const items = [
    { id: 'done-old', lane: 'done', since: 100 },
    { id: 'done-new', lane: 'done', since: 300 },
    { id: 'done-pinned-old', lane: 'done', since: 50, pinned: true },
    { id: 'done-unknown', lane: 'done', since: null },
    { id: 'run-late', lane: 'running', since: 500 },
    { id: 'run-early', lane: 'running', since: 200 },
    { id: 'run-pinned', lane: 'running', since: 900, pinned: true },
    { id: 'run-unknown', lane: 'running' },
    { id: 'wait-short', lane: 'attention', since: 900, pinned: true },
    { id: 'wait-long', lane: 'attention', since: 100 },
    { id: 'queue-2', lane: 'queued', queuePosition: 2, since: 1 },
    { id: 'queue-1', lane: 'queued', queuePosition: 1, since: 9 },
    { id: 'hidden', lane: null, since: 0 },
  ]
  assert.deepEqual(
    sortBoardItems(items).map((item) => item.id),
    [
      // 需要你只看等待时长，置顶不插队。
      'wait-long',
      'wait-short',
      'run-pinned',
      'run-early',
      'run-late',
      'run-unknown',
      'queue-1',
      'queue-2',
      'done-pinned-old',
      'done-new',
      'done-old',
      'done-unknown',
      'hidden',
    ],
  )
  assert.equal(items[0].id, 'done-old', 'sortBoardItems must not mutate its input')
  // 同一时刻以 id 决定次序，保证轮询前后卡片不抖动。
  assert.ok(
    compareBoardItems({ id: 'a', lane: 'done', since: 1 }, { id: 'b', lane: 'done', since: 1 }) < 0,
  )
  assert.equal(compareBoardItems({ id: 'a', lane: 'done' }, { id: 'a', lane: 'done' }), 0)
})
