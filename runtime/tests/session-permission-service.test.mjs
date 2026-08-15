import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
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
    /工作目录之外/,
  )
  assert.match(
    permissionRequirement({ mode: 'ask', cwd, toolName: 'write', args: { path: 'README.md' } })
      .reason,
    /需要确认/,
  )
  assert.match(
    permissionRequirement({
      mode: 'ask',
      executionMode: 'approval-required',
      cwd,
      toolName: 'skill_create',
      args: { name: 'project-helper', scope: 'project' },
    }).reason,
    /需要确认/,
  )
  assert.match(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd,
      toolName: 'skill_create',
      args: { name: 'global-helper', scope: 'global' },
    }).reason,
    /完全访问模式/,
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
    /需要确认/,
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
    /需要确认/,
  )
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
    /工作目录之外/,
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
    executionMode: 'read-only',
    cwd: workspace,
    toolName: 'write',
    args: { path: join(workspace, 'linked-outside', 'escaped.txt') },
  })
  assert.match(requirement.reason, /工作目录之外/)
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
  assert.deepEqual(await denied, { block: true, reason: '用户拒绝执行该工具。' })
  assert.ok(events.some((item) => item.event === 'permission_request'))
  assert.ok(events.some((item) => item.event === 'permission_resolved'))

  const outside = resolve(process.cwd(), '..', 'outside.txt')
  assert.deepEqual(
    await service.authorize({
      sessionId: 'session-1',
      cwd: process.cwd(),
      toolName: 'write',
      args: { path: outside },
    }),
    { block: true, reason: 'write 不能在当前执行模式下访问当前工作目录之外的文件。' },
  )
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
  assert.deepEqual(await repeated, { block: true, reason: '用户拒绝执行该工具。' })
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
    reason: '目标文件在审核期间发生了变化，请重新请求修改并查看最新 Diff。',
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
  assert.match(approval.reason, /命令守卫拦截/)
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
  assert.match(approval2.reason, /命令守卫拦截/)
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
    { block: true, risk: 'high', reason: '命令守卫拦截：递归删除根目录或家目录。' },
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
    { risk: 'high', reason: '命令守卫拦截：递归删除（rm -r）。请确认后执行。' },
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
