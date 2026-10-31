import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  COMMAND_GUARD_REASON_PREFIX,
  approvalSeverity,
  deriveApprovalSeverity,
} from '../../shared/session-board-status.mjs'
import { SessionPermissionService } from '../services/session-permission-service.mjs'
import { CARD_LIMITS, projectSessionCard } from '../services/session-card-summary.mjs'

async function waitForPending(service, sessionId, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const pending = service.getPending(sessionId)
    if (pending.length) return pending
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  assert.fail(`Timed out waiting for a pending approval in ${sessionId}`)
}

// 规格 3.5：同一份规则表在 Runtime（权限服务赋值）与前端（旧 Runtime 降级）两端给出同样的结论。
const CASES = [
  {
    name: 'command guard warn interception (recursive delete)',
    mode: 'auto',
    toolName: 'bash',
    args: { command: 'rm -r build' },
    severity: 'critical',
  },
  {
    name: 'command guard warn interception (hard reset)',
    mode: 'ask',
    toolName: 'bash',
    args: { command: 'git reset --hard HEAD~1' },
    severity: 'critical',
  },
  {
    name: 'plain shell command',
    mode: 'ask',
    toolName: 'bash',
    args: { command: 'npm test' },
    severity: 'standard',
  },
  {
    name: 'workspace edit',
    mode: 'ask',
    toolName: 'edit',
    args: { path: 'README.md', oldText: 'a', newText: 'b' },
    severity: 'standard',
    preview: true,
  },
  {
    name: 'global plugin installation',
    mode: 'auto',
    toolName: 'plugin_create',
    args: { name: 'helper' },
    severity: 'critical',
  },
  {
    // 负责人决定：组件写入按普通写文件处理，不算 critical（不需要按住确认）。
    name: 'native component write',
    mode: 'ask',
    toolName: 'component_create',
    args: { mode: 'create', id: 'ci-status' },
    severity: 'standard',
  },
  {
    name: 'browser interaction that may submit a form',
    mode: 'auto',
    toolName: 'browser_automation',
    args: { action: 'click', selector: '#submit' },
    severity: 'critical',
  },
]

test('permission requests carry the same severity the shared rule table derives', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-approval-severity-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  for (const item of CASES) {
    const events = []
    const observed = []
    const service = new SessionPermissionService({
      getMode: () => item.mode,
      getExecutionMode: () => 'workspace-write',
      timeoutMs: 5000,
      getFileChangePreview: async () => ({ path: 'README.md', diff: '-a\n+b', truncated: false }),
      onEvent: (sessionId, event, data) => observed.push({ sessionId, event, data }),
    })
    const sessionId = `session-${item.toolName}`
    service.attachEmitter(sessionId, (event, data) => events.push({ event, data }))
    const pending = service.authorize({
      sessionId,
      cwd: directory,
      toolName: item.toolName,
      toolCallId: 'call-1',
      args: item.args,
    })
    const [approval] = await waitForPending(service, sessionId)
    assert.equal(approval.severity, item.severity, item.name)
    // 前端对旧 Runtime 的降级判断（没有 severity 字段）与 Runtime 的赋值一致。
    const { severity: _severity, ...legacy } = approval
    assert.equal(approvalSeverity(legacy), item.severity, `${item.name} (legacy fallback)`)
    assert.equal(deriveApprovalSeverity(approval), item.severity, `${item.name} (rule table)`)
    if (item.args.command?.startsWith('rm -r'))
      assert.ok(approval.reason.startsWith(COMMAND_GUARD_REASON_PREFIX))
    const request = events.find((entry) => entry.event === 'permission_request')
    assert.equal(request.data.severity, item.severity, `${item.name} (event)`)
    // 观察者不依赖 emitter：请求与结算都会通知看板事件流。
    service.resolve(sessionId, approval.id, false)
    await pending
    assert.deepEqual(
      observed.map((entry) => entry.event),
      ['permission_request', 'permission_resolved'],
    )
    assert.ok(observed.every((entry) => entry.sessionId === sessionId))
  }
})

test('the board card shows severity, a bounded argument preview and a short reason', () => {
  const command = `${'echo 你好🙂 '.repeat(80)}&& rm -r build`
  const card = projectSessionCard({
    session: {
      id: 'session-approval',
      streaming: true,
      needsAttention: true,
      attentionReason: 'approval',
      modified: '2026-10-29T02:00:00.000Z',
    },
    approvals: [
      {
        id: 'approval-2',
        toolName: 'bash',
        args: { command },
        risk: 'high',
        severity: 'critical',
        reason: `${COMMAND_GUARD_REASON_PREFIX}：${'递归删除（rm -r）'.repeat(20)}。请确认后执行。`,
        createdAt: '2026-10-29T02:00:02.000Z',
      },
      {
        id: 'approval-1',
        toolName: 'edit',
        args: { path: 'src/app.ts', oldText: 'a\u001b[31m', newText: 'b' },
        risk: 'high',
        reason: 'edit 将修改当前工作区中的文件，需要确认后执行。',
        createdAt: '2026-10-29T02:00:01.000Z',
      },
      { toolName: 'bash', args: {} },
    ],
    now: Date.parse('2026-10-29T02:00:05.000Z'),
  })
  assert.equal(card.boardStatus, 'attention')
  assert.deepEqual(
    card.approvals.map((approval) => approval.id),
    ['approval-1', 'approval-2'],
  )
  const [edit, bash] = card.approvals
  assert.equal(edit.severity, 'standard')
  assert.equal(edit.argsPreview, 'path: src/app.ts\noldText: a\nnewText: b')
  assert.equal(Object.hasOwn(edit, 'argsTruncated'), false)
  assert.equal(bash.severity, 'critical')
  assert.equal(bash.argsTruncated, true)
  assert.equal([...bash.argsPreview].length, CARD_LIMITS.argsPreview)
  assert.ok(bash.argsPreview.endsWith('…'))
  // 按码点截断：不会把表情符号的代理对切成两半。
  assert.doesNotMatch(bash.argsPreview, /[\ud800-\udbff](?![\udc00-\udfff])/)
  assert.ok([...bash.reason].length <= CARD_LIMITS.reason)
  assert.ok(bash.reason.startsWith(COMMAND_GUARD_REASON_PREFIX))
})
