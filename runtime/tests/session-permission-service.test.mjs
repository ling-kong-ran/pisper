import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { guardCommand } from '../tools/command-guard.mjs'
import {
  permissionRequirement,
  SessionPermissionService,
} from '../services/session-permission-service.mjs'

async function waitForPending(service, sessionId, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const pending = service.getPending(sessionId)
    if (pending.length) return pending
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  assert.fail(`Timed out waiting for a pending approval in ${sessionId}`)
}

test('permission modes progress from ask to automatic to ignored checks', () => {
  const cwd = resolve('workspace')
  const outside = resolve(cwd, '..', 'outside.txt')
  assert.equal(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'read', args: { path: 'README.md' } }),
    null,
  )
  assert.match(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'read', args: { path: outside } }).reason,
    /outside the working folder/,
  )
  // Bot 的会话不围工作目录：目录外的文件照常按模式判定（读直接读，写在 ask 下要确认，auto 下直接写）。
  assert.equal(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'read',
      args: { path: outside },
      fenced: false,
    }),
    null,
  )
  assert.match(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'write',
      args: { path: outside },
      fenced: false,
    }).reason,
    /Needs confirmation/,
  )
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'write',
      args: { path: outside },
      fenced: false,
    }),
    null,
  )
  assert.match(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'write', args: { path: 'README.md' } })
      .reason,
    /Needs confirmation/,
  )
  assert.match(
    permissionRequirement({
      mode: 'ask',
      executionMode: 'approval-required',
      cwd,
      toolName: 'skill_create',
      args: { name: 'project-helper', scope: 'project' },
    }).reason,
    /Needs confirmation/,
  )
  assert.match(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'skill_create',
      args: { name: 'global-helper', scope: 'global' },
    }).reason,
    /global skill.*Needs confirmation/,
  )
  for (const [mode, executionMode] of [
    ['ask', 'approval-required'],
    ['auto', 'workspace-write'],
  ])
    assert.equal(
      permissionRequirement({
        mode,
        executionMode,
        cwd,
        toolName: 'skill_create',
        args: { name: 'global-helper', scope: 'global' },
      }).block,
      undefined,
      executionMode,
    )
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'full-access',
      cwd,
      toolName: 'skill_create',
      args: { name: 'global-helper', scope: 'global' },
    }),
    null,
  )
  assert.match(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'spawn_agent',
      args: { message: 'inspect the repository' },
    }).reason,
    /Needs confirmation/,
  )
  assert.equal(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'mcp_read_123',
      toolRisk: 'low',
      args: {},
    }),
    null,
  )
  assert.match(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'mcp_write_456',
      toolRisk: 'high',
      args: {},
    }).reason,
    /Needs confirmation/,
  )
  for (const action of [
    'get_device_info',
    'get_capabilities',
    'get_battery_status',
    'get_storage_status',
    'get_memory_status',
    'get_network_status',
    'get_display_status',
    'get_locale_status',
    'get_device_status',
    'delete_photos',
    'open_app',
  ]) {
    assert.equal(
      permissionRequirement({
        mode: 'ask',
        executionMode: 'approval-required',
        cwd,
        toolName: 'mobile_device',
        toolRisk: 'high',
        args: { action },
      }),
      null,
    )
  }
  assert.equal(
    permissionRequirement({
      mode: 'ask',
      cwd,
      toolName: 'update_goal',
      args: { status: 'complete' },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'update_plan', args: { items: [] } }),
    null,
  )
  assert.equal(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'update_task_list', args: { items: [] } }),
    null,
  )
  assert.equal(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'write', args: { path: 'README.md' } }),
    null,
  )
  assert.match(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'write', args: { path: outside } }).reason,
    /outside the working folder/,
  )
  // 执行模式只决定是否逐次确认：工作区之外的文件是“确认后执行”，不是直接拒绝。
  for (const mode of ['ask', 'auto'])
    for (const toolName of ['read', 'ls', 'grep', 'find', 'edit', 'write'])
      assert.equal(
        permissionRequirement({ mode, cwd, toolName, args: { path: outside } }).block,
        undefined,
        `${mode} ${toolName}`,
      )
  assert.equal(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'bash', args: { command: 'npm test' } }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'full-access',
      cwd,
      toolName: 'bash',
      args: { command: 'date' },
    }),
    null,
  )
})

test('mobile_device never creates a generic approval request', async () => {
  const service = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'approval-required',
  })
  assert.equal(
    await service.authorize({
      sessionId: 'session-mobile',
      cwd: process.cwd(),
      toolName: 'mobile_device',
      toolCallId: 'mobile-call-1',
      args: { action: 'get_storage_status' },
    }),
    undefined,
  )
  assert.deepEqual(service.getPending('session-mobile'), [])
})

test('path checks resolve symbolic links before authorization', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-permission-path-'))
  const workspace = join(directory, 'workspace')
  const outside = join(directory, 'outside')
  await mkdir(workspace)
  await mkdir(outside)
  await symlink(
    outside,
    join(workspace, 'linked-outside'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  t.after(() => rm(directory, { recursive: true, force: true }))

  const requirement = permissionRequirement({
    mode: 'auto',
    executionMode: 'approval-required',
    cwd: workspace,
    toolName: 'write',
    args: { path: join(workspace, 'linked-outside', 'escaped.txt') },
  })
  assert.match(requirement.reason, /outside the working folder/)
})

test('pending tool approval can be accepted or denied', async () => {
  let mode = 'ask'
  let executionMode = ''
  const events = []
  const service = new SessionPermissionService({
    getMode: () => mode,
    getExecutionMode: () => executionMode,
    timeoutMs: 5000,
  })
  service.attachEmitter('session-1', (event, data) => events.push({ event, data }))

  assert.equal(
    await service.authorize({
      sessionId: 'session-1',
      cwd: process.cwd(),
      toolName: 'read',
      toolCallId: 'tool-read',
      args: { path: 'file.txt' },
    }),
    undefined,
  )
  assert.deepEqual(service.getPending('session-1'), [])

  const allowed = service.authorize({
    sessionId: 'session-1',
    cwd: process.cwd(),
    toolName: 'bash',
    toolCallId: 'tool-1',
    args: { command: 'npm test' },
  })
  await new Promise((resolve) => setImmediate(resolve))
  const first = service.getPending('session-1')[0]
  assert.equal(first.toolName, 'bash')
  const firstResolution = service.resolve('session-1', first.id, true)
  assert.equal(firstResolution.found, true)
  assert.equal(firstResolution.alreadyResolved, false)
  assert.equal(firstResolution.approved, true)
  assert.equal(await allowed, undefined)

  const duplicateResolution = service.resolve('session-1', first.id, true)
  assert.equal(duplicateResolution.found, true)
  assert.equal(duplicateResolution.alreadyResolved, true)
  assert.equal(duplicateResolution.approved, true)
  assert.equal(service.resolve('another-session', first.id, true).found, false)
  assert.equal(
    await service.authorize({
      sessionId: 'session-1',
      cwd: process.cwd(),
      toolName: 'bash',
      toolCallId: 'tool-duplicate',
      args: { command: 'npm test' },
    }),
    undefined,
  )
  assert.deepEqual(service.getPending('session-1'), [])

  const denied = service.authorize({
    sessionId: 'session-1',
    cwd: process.cwd(),
    toolName: 'bash',
    toolCallId: 'tool-2',
    args: { command: 'npm run check' },
  })
  await new Promise((resolve) => setImmediate(resolve))
  const second = service.getPending('session-1')[0]
  service.resolve('session-1', second.id, false)
  assert.deepEqual(await denied, { block: true, reason: 'Not run: the user denied this tool.' })
  assert.ok(events.some((item) => item.event === 'permission_request'))
  assert.ok(events.some((item) => item.event === 'permission_resolved'))

  // 工作区之外的写入走审批（带 ../ 相对路径的 Diff），批准后放行，不再直接拒绝。
  const outside = resolve(process.cwd(), '..', 'pisper-outside-preview.txt')
  const outsideWrite = service.authorize({
    sessionId: 'session-1',
    cwd: process.cwd(),
    toolName: 'write',
    toolCallId: 'tool-outside',
    args: { path: outside, content: 'hello\n' },
  })
  // 预览要异步读文件：等到审批请求真正入队，而不是固定等待一段时间。
  while (!service.getPending('session-1').length)
    await new Promise((resolve) => setImmediate(resolve))
  const outsideRequest = service.getPending('session-1')[0]
  assert.equal(outsideRequest.toolName, 'write')
  assert.match(outsideRequest.reason, /outside the working folder.*Needs confirmation/)
  assert.equal(outsideRequest.fileChange.path, '../pisper-outside-preview.txt')
  service.resolve('session-1', outsideRequest.id, true)
  assert.equal(await outsideWrite, undefined)
  assert.deepEqual(service.getPending('session-1'), [])

  mode = 'ignore'
  executionMode = 'full-access'
  assert.equal(
    await service.authorize({
      sessionId: 'session-1',
      cwd: process.cwd(),
      toolName: 'bash',
      args: { command: 'date' },
    }),
    undefined,
  )
  service.dispose()
})

test('approval-required file changes include a diff and are not remembered', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-permission-file-change-'))
  const path = join(directory, 'example.txt')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, 'before\n', 'utf8')
  const service = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'approval-required',
    timeoutMs: 5000,
  })

  const first = service.authorize({
    sessionId: 'session-file-change',
    cwd: directory,
    toolName: 'write',
    toolCallId: 'write-preview',
    args: { path: 'example.txt', content: 'after\n' },
  })
  const [approval] = await waitForPending(service, 'session-file-change')
  assert.equal(approval.fileChange.path, 'example.txt')
  assert.match(approval.fileChange.diff, /^diff --git a\/example\.txt b\/example\.txt$/m)
  assert.equal(Object.hasOwn(approval.fileChange, 'sourceHash'), false)
  service.resolve('session-file-change', approval.id, true)
  assert.equal(await first, undefined)

  const repeated = service.authorize({
    sessionId: 'session-file-change',
    cwd: directory,
    toolName: 'write',
    toolCallId: 'write-preview-again',
    args: { path: 'example.txt', content: 'after\n' },
  })
  const repeatedApprovals = await waitForPending(service, 'session-file-change')
  assert.equal(repeatedApprovals.length, 1)
  service.resolve('session-file-change', repeatedApprovals[0].id, false)
  assert.deepEqual(await repeated, { block: true, reason: 'Not run: the user denied this tool.' })
  service.dispose()
})

test('approval-required changes are rejected when the source file changes during review', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-permission-file-revision-'))
  const path = join(directory, 'example.txt')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, 'before\n', 'utf8')
  const service = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'approval-required',
    timeoutMs: 5000,
  })
  const pending = service.authorize({
    sessionId: 'session-file-revision',
    cwd: directory,
    toolName: 'write',
    toolCallId: 'write-revision',
    args: { path: 'example.txt', content: 'after\n' },
  })
  const [approval] = await waitForPending(service, 'session-file-revision')
  await writeFile(path, 'changed elsewhere\n', 'utf8')
  service.resolve('session-file-revision', approval.id, true)
  assert.deepEqual(await pending, {
    block: true,
    reason:
      'Not run: the target file changed during review. Request the change again to see the latest diff.',
  })
  service.dispose()
})

test('approved commands persist on disk and are reused after service recreation', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-permission-memory-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const approvalPath = join(directory, 'pisper-approvals.json')
  const options = { approvalPath, getMode: () => 'ask', timeoutMs: 5000 }
  const firstService = new SessionPermissionService(options)
  const allowed = firstService.authorize({
    sessionId: 'session-persisted',
    cwd: directory,
    toolName: 'bash',
    toolCallId: 'tool-persisted',
    args: { command: 'npm test' },
  })
  const [approval] = await waitForPending(firstService, 'session-persisted')
  firstService.resolve('session-persisted', approval.id, true)
  assert.equal(await allowed, undefined)
  firstService.dispose()

  const persisted = JSON.parse(await readFile(approvalPath, 'utf8'))
  const record = Object.values(persisted.approvals)[0]
  assert.equal(record.toolName, 'bash')
  assert.equal(record.cwd, directory)
  assert.equal(record.command, 'npm test')

  const secondService = new SessionPermissionService(options)
  assert.equal(
    await secondService.authorize({
      sessionId: 'another-session-in-the-same-directory',
      cwd: directory,
      toolName: 'bash',
      toolCallId: 'tool-reused',
      args: { command: 'npm test' },
    }),
    undefined,
  )
  assert.deepEqual(secondService.getPending('another-session-in-the-same-directory'), [])
  secondService.dispose()
})

test('approved dangerous bash commands are reused even with a different timeout', async () => {
  const service = new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => 'workspace-write',
    timeoutMs: 5000,
  })
  const cwd = resolve('workspace')

  // First run of a dangerous command requires approval.
  const first = service.authorize({
    sessionId: 'session-danger',
    cwd,
    toolName: 'bash',
    toolCallId: 'danger-1',
    args: { command: 'rm -rf node_modules', timeout: 30 },
  })
  const [approval] = await waitForPending(service, 'session-danger')
  assert.match(approval.reason, /^Blocked by the command guard: /)
  service.resolve('session-danger', approval.id, true)
  assert.equal(await first, undefined)

  // The same command with a different timeout reuses the approval (no prompt).
  assert.equal(
    await service.authorize({
      sessionId: 'session-danger',
      cwd,
      toolName: 'bash',
      toolCallId: 'danger-2',
      args: { command: 'rm -rf node_modules', timeout: 60 },
    }),
    undefined,
  )
  assert.deepEqual(service.getPending('session-danger'), [])

  // A different dangerous command still requires its own approval.
  const other = service.authorize({
    sessionId: 'session-danger',
    cwd,
    toolName: 'bash',
    toolCallId: 'danger-3',
    args: { command: 'rm -rf dist' },
  })
  const [approval2] = await waitForPending(service, 'session-danger')
  assert.match(approval2.reason, /^Blocked by the command guard: /)
  service.resolve('session-danger', approval2.id, true)
  assert.equal(await other, undefined)
  service.dispose()
})

test('approval API treats a repeated resolution as an idempotent success', async () => {
  const calls = []
  const handler = createApiHandler({
    resolveToolApproval(sessionId, approvalId, approved) {
      calls.push({ sessionId, approvalId, approved })
      return {
        found: true,
        alreadyResolved: true,
        id: approvalId,
        sessionId,
        approved: false,
        reason: '等待授权超时。',
      }
    },
  })
  const request = {
    method: 'POST',
    async *[Symbol.asyncIterator]() {
      yield Buffer.from(JSON.stringify({ approved: true }))
    },
  }
  const response = {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    end(body) {
      this.body = body
    },
  }
  const handled = await handler(
    request,
    response,
    new URL('http://localhost/api/sessions/session-1/approvals/approval-1'),
  )
  assert.equal(handled, true)
  assert.equal(response.status, 200)
  const body = JSON.parse(response.body)
  assert.equal(body.alreadyResolved, true)
  assert.equal(body.approved, false)
  assert.deepEqual(calls, [{ sessionId: 'session-1', approvalId: 'approval-1', approved: true }])
})

test('session hook preserves upstream tool blockers before permission checks', async () => {
  const session = {
    agent: { beforeToolCall: async () => ({ block: true, reason: 'extension blocked' }) },
  }
  const service = new SessionPermissionService({ getMode: () => 'ask' })
  service.install(session, { sessionId: 'session-hook', cwd: process.cwd() })
  const result = await session.agent.beforeToolCall({
    toolCall: { id: 'tool-hook', name: 'write' },
    args: { path: 'file.txt', content: 'content' },
  })
  assert.deepEqual(result, { block: true, reason: 'extension blocked' })
  assert.deepEqual(service.getPending('session-hook'), [])
  service.dispose()
})

test('command guard blocks catastrophic bash and routes recursive rm to approval', () => {
  const cwd = resolve('workspace')
  // Catastrophic command is blocked outright.
  assert.deepEqual(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'bash',
      args: { command: 'rm -rf /' },
    }),
    {
      block: true,
      risk: 'high',
      reason: `Blocked by the command guard: ${guardCommand('rm -rf /').reason}.`,
    },
  )
  // Relative recursive rm requires approval even in automatic mode.
  assert.deepEqual(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'bash',
      args: { command: 'rm -rf node_modules' },
    }),
    // guarded：持久允许规则不能替用户确认守卫标出的危险命令。
    {
      risk: 'high',
      reason: `Blocked by the command guard: ${guardCommand('rm -rf node_modules').reason}. Needs confirmation.`,
      guarded: true,
    },
  )
  // Ordinary bash stays approval-free in automatic mode.
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'bash',
      args: { command: 'ls -la' },
    }),
    null,
  )
})

test('pending approvals carry severity and notify the board observer without an attached emitter', async () => {
  const observed = []
  const service = new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => 'workspace-write',
    timeoutMs: 5000,
    onEvent: (sessionId, event, data) => observed.push({ sessionId, event, id: data.id }),
  })
  const cwd = resolve('workspace')
  const guarded = service.authorize({
    sessionId: 'session-background',
    cwd,
    toolName: 'bash',
    toolCallId: 'guarded-1',
    args: { command: 'git clean -fd' },
  })
  const [approval] = await waitForPending(service, 'session-background')
  assert.equal(approval.severity, 'critical')
  assert.equal(service.getPending('session-background')[0].severity, 'critical')
  service.resolve('session-background', approval.id, false)
  await guarded
  assert.deepEqual(observed, [
    { sessionId: 'session-background', event: 'permission_request', id: approval.id },
    { sessionId: 'session-background', event: 'permission_resolved', id: approval.id },
  ])
  // 观察者抛错不影响审批流程。
  const throwing = new SessionPermissionService({
    getMode: () => 'ask',
    timeoutMs: 5000,
    onEvent: () => {
      throw new Error('observer failed')
    },
  })
  const plain = throwing.authorize({
    sessionId: 'session-plain',
    cwd,
    toolName: 'bash',
    toolCallId: 'plain-1',
    args: { command: 'npm test' },
  })
  const [standard] = await waitForPending(throwing, 'session-plain')
  assert.equal(standard.severity, 'standard')
  throwing.resolve('session-plain', standard.id, true)
  assert.equal(await plain, undefined)
})

test('bot sessions edit files outside their folder without the folder fence', async () => {
  const service = new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => 'workspace-write',
    isUnfenced: (sessionId) => sessionId === 'bot-thread',
  })
  const outside = resolve('/repo', '..', 'notes', 'todo.md')
  assert.equal(
    await service.authorize({
      sessionId: 'bot-thread',
      cwd: '/repo',
      toolName: 'write',
      toolCallId: 'call-1',
      args: { path: outside },
    }),
    undefined,
  )
  // 普通会话访问工作目录之外要先确认（模式只决定是否询问，不拒绝）；Bot 的会话不围。
  const fenced = permissionRequirement({
    mode: 'auto',
    executionMode: 'workspace-write',
    cwd: '/repo',
    toolName: 'write',
    args: { path: outside },
  })
  assert.equal(fenced.block, undefined)
  assert.match(fenced.reason, /outside the working folder.*confirmation/)
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd: '/repo',
      toolName: 'write',
      args: { path: outside },
      fenced: false,
    }),
    null,
  )
})

test('tools outside the run scope are blocked before any mode, rule or approval', async () => {
  const blocked = new Set(['memory_search'])
  const service = new SessionPermissionService({
    getMode: () => 'full-access',
    getExecutionMode: () => 'full-access',
    rules: { evaluate: async () => ({ effect: 'allow', pattern: '*' }) },
    isToolBlocked: (sessionId, toolName) => sessionId === 'bot-thread' && blocked.has(toolName),
  })
  const result = await service.authorize({
    sessionId: 'bot-thread',
    cwd: '/repo',
    toolName: 'memory_search',
    toolCallId: 'call-1',
    args: { query: 'x' },
  })
  assert.equal(result.block, true)
  assert.match(result.reason, /memory_search/)
  assert.equal(service.getPending('bot-thread').length, 0)
  // 其他会话、其他工具不受影响。
  assert.equal(
    await service.authorize({
      sessionId: 'other',
      cwd: '/repo',
      toolName: 'memory_search',
      toolCallId: 'call-2',
      args: {},
    }),
    undefined,
  )
})
