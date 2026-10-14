import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { MultiAgentService } from '../services/multi-agent-service.mjs'
import { TeamWorkflowService } from '../services/team-workflow.mjs'
import { factories } from '../tools/app/multi-agent.mjs'

// —— 测试夹具 ——

function deferred() {
  let resolve
  let reject
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

async function waitFor(predicate, message = 'condition') {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  throw new Error(`Timed out waiting for ${message}.`)
}

function createFakeSession({ onPrompt } = {}) {
  const listeners = new Set()
  const session = {
    agent: { state: { systemPrompt: 'Base system prompt' } },
    messages: [],
    promptCalls: [],
    steerCalls: [],
    followUpCalls: [],
    aborted: false,
    disposed: false,
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    emit(event) {
      for (const listener of listeners) listener(event)
    },
    async prompt(message) {
      session.promptCalls.push(message)
      await onPrompt?.({ message, session })
    },
    async steer(message) {
      session.steerCalls.push(message)
    },
    async followUp(message) {
      session.followUpCalls.push(message)
    },
    async abort() {
      session.aborted = true
    },
    dispose() {
      session.disposed = true
    },
  }
  return session
}

function createService(session, overrides = {}) {
  const service = new MultiAgentService({
    getModelRuntime: () => ({ id: 'runtime' }),
    getSettingsManager: () => ({ id: 'settings' }),
    createResourceLoader: async (options) => ({ options }),
    createSessionManager: () => ({ appendMessage() {} }),
    createSession: async () => ({ session }),
    ...overrides,
  })
  return service
}

function baseInput(overrides = {}) {
  return {
    parentSessionId: 'parent-1',
    cwd: process.cwd(),
    model: { provider: 'openai', id: 'gpt-5', contextWindow: 200_000, maxTokens: 128_000 },
    thinkingLevel: 'high',
    taskName: 'interrupt_recovery',
    message: 'Finish the delegated task.',
    allowedTools: ['read', 'bash'],
    customTools: [],
    ...overrides,
  }
}

// —— canComplete：终态未完成任务要给出恢复路径 ——

test('canComplete distinguishes active blockers from terminal blockers with recovery guidance', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-recovery-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { goalId: 'goal-1', objective: 'Ship it.', tokenBudget: 100 })

  // 运行中任务：保留原有「未完成工作流」文案，不带恢复指引。
  const running = await service.registerTask('session-1', {
    taskName: 'running-task',
    files: ['src/a.ts'],
    message: 'Work.',
  })
  await service.bindAgent('session-1', running.id, { id: 'agent-run', status: 'running' })
  const activeBlock = service.canComplete('session-1')
  assert.equal(activeBlock.ok, false)
  assert.match(activeBlock.reason, /running-task \(running\)/)
  assert.doesNotMatch(activeBlock.reason, /followup_task/)

  // 中断任务：文案必须给出 followup / 同名重启两条恢复路径。
  await service.updateAgent('session-1', { id: 'agent-run', status: 'interrupted' })
  const terminalBlock = service.canComplete('session-1')
  assert.equal(terminalBlock.ok, false)
  assert.match(terminalBlock.reason, /running-task \(interrupted\)/)
  assert.match(terminalBlock.reason, /followup_task/)
  assert.match(terminalBlock.reason, /相同 taskName/)
})

// —— list_agents：活跃 Agent 附带空闲时长 ——

test('list_agents surfaces idle time for active Agents only', async () => {
  const staleAt = new Date(Date.now() - 12 * 60_000).toISOString()
  const freshAt = new Date().toISOString()
  const listTool = factories.list_agents({
    multiAgentRuntime: {
      list: async () => [
        { canonicalName: '/root/stuck_1', status: 'running', lastActivityAt: staleAt },
        { canonicalName: '/root/busy_2', status: 'running', lastActivityAt: freshAt },
        {
          canonicalName: '/root/done_3',
          status: 'completed',
          lastActivityAt: staleAt,
          durationMs: 691_700,
        },
      ],
    },
  })
  const result = await listTool.execute('call-1', {})
  const text = result.content.map((block) => block.text).join('\n')
  // 卡死的活跃 Agent 必须暴露空闲时长，供 lead 判断是否接管。
  assert.match(text, /\/root\/stuck_1 · running, idle \d+s/)
  // 刚活动过的 Agent 不附带空闲时长。
  assert.match(text, /\/root\/busy_2 · running\n/)
  // 终态 Agent 只显示运行时长，不显示空闲时长。
  assert.match(text, /\/root\/done_3 · completed, 691\.7s/)
  assert.doesNotMatch(text, /done_3[^\n]*idle/)
})

// —— interrupt_agent：返回附工作区中间态警示 ——

test('interrupt_agent warns that the workspace may hold partial edits', async () => {
  const interruptTool = factories.interrupt_agent({
    multiAgentRuntime: {
      interrupt: async () => ({ canonicalName: '/root/split_1', status: 'interrupted' }),
    },
  })
  const result = await interruptTool.execute('call-1', { target: 'split' })
  const text = result.content.map((block) => block.text).join('\n')
  assert.match(text, /\/root\/split_1 is interrupted\./)
  assert.match(text, /partial multi-file edits/)
  assert.match(text, /respawning instead of messaging/)

  // 非中断结果（如已在终态）不追加警示。
  const noopTool = factories.interrupt_agent({
    multiAgentRuntime: {
      interrupt: async () => ({ canonicalName: '/root/done_1', status: 'completed' }),
    },
  })
  const noopResult = await noopTool.execute('call-2', { target: 'done' })
  const noopText = noopResult.content.map((block) => block.text).join('\n')
  assert.equal(noopText, '/root/done_1 is completed.')
})

// —— followup：过期错误给出可操作恢复路径 ——

test('followup after context expiry points at respawning with the same taskName', async () => {
  const gate = deferred()
  const session = createFakeSession({
    onPrompt: async ({ session: active }) => {
      await gate.promise
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'working' }],
        usage: { input: 1, output: 1, totalTokens: 2 },
      })
    },
  })
  // retention 设为 0：终态后立即释放会话，模拟上下文过期。
  const service = createService(session, { terminalSessionRetentionMs: 0 })
  const started = await service.spawn(baseInput())
  await waitFor(() => service.list('parent-1')[0]?.status === 'running', 'Agent starts running')
  service.interrupt('parent-1', started.id)
  // 释放 prompt 闸门，让被中断的运行能够落定，避免 dispose 挂起。
  gate.resolve()
  await waitFor(() => service.list('parent-1')[0]?.status === 'interrupted', 'interrupt settles')
  // 手动触发保留期释放，使 record.session 置空。
  const record = service.records.get(started.id)
  record.session = null
  await assert.rejects(service.followup('parent-1', started.id, 'Continue.'), (error) => {
    assert.match(error.message, /context expired/)
    // 错误必须告诉调用方如何恢复，而不是死胡同。
    assert.match(error.message, /same taskName/)
    return true
  })
  await service.dispose()
})
