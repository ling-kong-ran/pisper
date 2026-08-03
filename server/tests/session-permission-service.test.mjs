import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, symlink } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import {
  permissionRequirement,
  SessionPermissionService,
} from '../services/session-permission-service.mjs'

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
  assert.match(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'write', args: { path: 'README.md' } })
      .reason,
    /修改当前工作区/,
  )
  assert.match(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'write', args: { path: outside } }).reason,
    /工作目录之外/,
  )
  assert.match(
    permissionRequirement({ mode: 'auto', cwd, toolName: 'bash', args: { command: 'npm test' } })
      .reason,
    /Shell/,
  )
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace',
      cwd,
      toolName: 'write',
      args: { path: 'README.md' },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace',
      cwd,
      toolName: 'edit',
      args: { path: 'README.md', edits: [] },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace',
      cwd,
      toolName: 'browser_automation',
      toolRisk: 'high',
      args: { action: 'click' },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'workspace',
      cwd,
      toolName: 'write',
      args: { path: outside },
    }).block,
    true,
  )
  assert.match(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'workspace',
      cwd,
      toolName: 'bash',
      args: { command: 'date' },
    }).reason,
    /Shell/,
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

test('workspace path checks resolve symbolic links before authorization', async (t) => {
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
    executionMode: 'workspace',
    cwd: workspace,
    toolName: 'write',
    args: { path: join(workspace, 'linked-outside', 'escaped.txt') },
  })
  assert.match(requirement.reason, /工作目录之外/)
})

test('pending tool approval can be accepted or denied', async () => {
  let mode = 'ask'
  let executionMode = 'workspace'
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
      toolName: 'write',
      toolCallId: 'tool-write',
      args: { path: 'file.txt', content: 'ok' },
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

  const denied = service.authorize({
    sessionId: 'session-1',
    cwd: process.cwd(),
    toolName: 'bash',
    toolCallId: 'tool-2',
    args: { command: 'npm test' },
  })
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
    { block: true, reason: 'write 不能在工作区模式下访问当前工作目录之外的文件。' },
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
