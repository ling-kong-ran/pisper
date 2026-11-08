// Bot 对话的命令不阻塞（v2.1）：时限内跑完照常返回；超过时限或这一步被新消息打断时转后台接着跑，
// 跑完把结果交回对话；时间线里能看到、能停。要批准的操作在对话里不等，直接退回。
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  BotCommandJobs,
  botCommandResultPrompt,
  isBotCommandResultMessage,
} from '../runtime/bot-background-commands.mjs'
import { isInternalParentMessage } from '../runtime/stream-projection.mjs'
import { installStepInterrupt, interruptCurrentStep } from '../runtime/step-interrupt.mjs'
import {
  BOT_CONVERSATION_BLOCKED_TOOLS,
  botConversationApprovalRefusal,
  runToolBlocker,
} from '../runtime/tool-activation.mjs'
import {
  NO_ONE_TO_APPROVE_REASON,
  NO_WAIT_APPROVAL_REASON,
  SessionPermissionService,
} from '../services/session-permission-service.mjs'
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'

// 假的 bash：等 ms 毫秒后返回输出；收到中止信号时报错（和真的一样会被杀掉）。
function fakeBash() {
  const calls = []
  return {
    calls,
    tool: {
      name: 'bash',
      description: 'Execute a bash command',
      execute: (_id, params, signal, onUpdate) =>
        new Promise((resolve, reject) => {
          calls.push(params.command)
          onUpdate?.({ content: [{ type: 'text', text: 'partial' }] })
          const timer = setTimeout(
            () => resolve({ content: [{ type: 'text', text: `done: ${params.command}` }] }),
            Number(params.ms) || 0,
          )
          signal?.addEventListener('abort', () => {
            clearTimeout(timer)
            reject(new Error('killed'))
          })
        }),
    },
  }
}

test('a quick command returns as usual', async () => {
  const jobs = new BotCommandJobs({ backgroundAfterMs: 200 })
  const bash = fakeBash()
  const tool = jobs.wrap(bash.tool, { sessionId: 'conv' })
  assert.match(tool.description, /keeps running in the background/)
  const result = await tool.execute('t1', { command: 'ls', ms: 5 })
  assert.equal(result.content[0].text, 'done: ls')
  assert.deepEqual(jobs.list('conv'), [])
})

test('a slow command moves to the background and its result comes back later', async () => {
  const finished = []
  const jobs = new BotCommandJobs({
    backgroundAfterMs: 30,
    onFinished: (sessionId, job) => finished.push([sessionId, job]),
  })
  const tool = jobs.wrap(fakeBash().tool, { sessionId: 'conv' })
  const result = await tool.execute('t1', { command: 'npm test', ms: 120 })
  assert.match(result.content[0].text, /moved to the background as cmd_/)
  const [job] = jobs.list('conv')
  assert.equal(job.kind, 'command')
  assert.equal(job.status, 'running')
  assert.equal(jobs.hasActive('conv'), true)
  await new Promise((resolve) => setTimeout(resolve, 150))
  assert.equal(finished.length, 1)
  assert.equal(finished[0][0], 'conv')
  assert.equal(finished[0][1].status, 'completed')
  assert.equal(finished[0][1].output, 'done: npm test')
  const prompt = botCommandResultPrompt(finished[0][1])
  assert.equal(isBotCommandResultMessage(prompt), true)
  assert.equal(isInternalParentMessage(prompt), true)
  assert.match(prompt, /`npm test`\) finished/)
})

test('a new message interrupting the step moves the command to the background instead of killing it', async () => {
  const finished = []
  const jobs = new BotCommandJobs({
    backgroundAfterMs: 10_000,
    onFinished: (_sessionId, job) => finished.push(job),
  })
  const tool = jobs.wrap(fakeBash().tool, { sessionId: 'conv' })
  const step = new AbortController()
  const pending = tool.execute('t1', { command: 'npm run build', ms: 60 }, step.signal)
  step.abort(new Error('interrupted'))
  const result = await pending
  assert.match(result.content[0].text, /when the user sent a new message/)
  await new Promise((resolve) => setTimeout(resolve, 90))
  assert.equal(finished[0].status, 'completed')
})

test('a background command can be stopped from the timeline', async () => {
  const finished = []
  const jobs = new BotCommandJobs({
    backgroundAfterMs: 10,
    onFinished: (_sessionId, job) => finished.push(job),
  })
  const tool = jobs.wrap(fakeBash().tool, { sessionId: 'conv' })
  await tool.execute('t1', { command: 'sleep 100', ms: 10_000 })
  const [job] = jobs.list('conv')
  assert.equal(jobs.stop('other', job.id), null)
  assert.equal(jobs.stop('conv', job.id).status, 'interrupted')
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(finished[0].status, 'interrupted')
  assert.equal(jobs.hasActive('conv'), false)
})

test('a bot conversation never waits for approval: the step comes back telling it to hand off', async () => {
  const permissions = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'approval-required',
  })
  const blocked = await permissions.authorize({
    sessionId: 'conv',
    cwd: '/repo',
    toolName: 'bash',
    toolCallId: 't1',
    args: { command: 'npm install left-pad' },
    noWait: true,
  })
  assert.deepEqual(blocked, { block: true, reason: NO_WAIT_APPROVAL_REASON })
  assert.match(NO_WAIT_APPROVAL_REASON, /bot_job_start/)
  assert.deepEqual(permissions.getPending('conv'), [])
  // 经理工具本身不要批准。
  assert.equal(
    await permissions.authorize({
      sessionId: 'conv',
      cwd: '/repo',
      toolName: 'bot_job_start',
      toolCallId: 't2',
      args: {},
      noWait: true,
    }),
    undefined,
  )
})

test('a bot conversation can use every tool except those with a bot_* replacement', () => {
  const blocked = runToolBlocker({ blockedToolNames: BOT_CONVERSATION_BLOCKED_TOOLS })
  for (const name of [
    'browser_automation',
    'computer_use',
    'workflow_list',
    'workflow_run',
    'get_goal',
    'update_goal',
  ])
    assert.equal(blocked(name), false, name)
  for (const name of ['ask_user', 'spawn_agent', 'wait_agent', 'schedule_manage'])
    assert.equal(blocked(name), true, name)
  // Bot 设定里自定义的工具范围仍然有效。
  const scoped = runToolBlocker({
    blockedToolNames: BOT_CONVERSATION_BLOCKED_TOOLS,
    allowedToolNames: ['read'],
  })
  assert.equal(scoped('browser_automation'), true)
  assert.equal(scoped('computer_use'), true)
})

test('approvals wait in the conversation only for tools a sub-agent cannot run, and only when someone can answer', () => {
  const refusal = (toolName, flags = {}) => botConversationApprovalRefusal({ toolName, ...flags })
  assert.equal(refusal('bash'), NO_WAIT_APPROVAL_REASON)
  assert.equal(refusal('edit'), NO_WAIT_APPROVAL_REASON)
  for (const toolName of ['browser_automation', 'computer_use', 'workflow_run']) {
    assert.equal(refusal(toolName), '', toolName)
    assert.equal(refusal(toolName, { unattended: true }), NO_ONE_TO_APPROVE_REASON, toolName)
    assert.equal(refusal(toolName, { quiet: true }), NO_ONE_TO_APPROVE_REASON, toolName)
  }
  // 子 agent 做不了，就不能再叫它“交给子 agent”。
  assert.doesNotMatch(NO_ONE_TO_APPROVE_REASON, /bot_job_start/)
})

function botPermissions() {
  return new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => 'workspace-write',
  })
}

const botApprovalInput = (toolName, args, flags = {}) => ({
  sessionId: 'conv',
  cwd: '/repo',
  toolName,
  toolCallId: `call-${toolName}`,
  args,
  noWait: (name) => botConversationApprovalRefusal({ toolName: name, ...flags }),
})

async function waitForPending(permissions) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const [pending] = permissions.getPending('conv')
    if (pending) return pending
    await new Promise((resolve) => setImmediate(resolve))
  }
  throw new Error('the approval card never appeared')
}

test('browser clicks in a bot conversation show a card and run once approved', async () => {
  const permissions = botPermissions()
  // 只是打开网页，不要批准。
  assert.equal(
    await permissions.authorize(
      botApprovalInput('browser_automation', { action: 'open', url: 'https://example.com' }),
    ),
    undefined,
  )
  const click = permissions.authorize(
    botApprovalInput('browser_automation', { action: 'click', selector: '#go' }),
  )
  const pending = await waitForPending(permissions)
  assert.equal(pending.toolName, 'browser_automation')
  permissions.resolve('conv', pending.id, true)
  assert.equal(await click, undefined)
})

test('nobody around: browser clicks are refused at once without leaving a card', async () => {
  const permissions = botPermissions()
  assert.deepEqual(
    await permissions.authorize(
      botApprovalInput(
        'browser_automation',
        { action: 'click', selector: '#go' },
        { unattended: true },
      ),
    ),
    { block: true, reason: NO_ONE_TO_APPROVE_REASON },
  )
  assert.deepEqual(permissions.getPending('conv'), [])
})

test('computer use approval in a bot conversation lasts one run, not the whole long-lived conversation', async () => {
  const permissions = botPermissions()
  let run = 'run-1'
  const input = () => ({
    ...botApprovalInput('computer_use', { action: 'screenshot' }),
    approvalRun: () => run,
  })
  const first = permissions.authorize(input())
  permissions.resolve('conv', (await waitForPending(permissions)).id, true)
  assert.equal(await first, undefined)
  // 同一轮不再问。
  assert.equal(await permissions.authorize(input()), undefined)
  assert.deepEqual(permissions.getPending('conv'), [])
  // 下一轮要重新批准。
  run = 'run-2'
  const next = permissions.authorize(input())
  permissions.resolve('conv', (await waitForPending(permissions)).id, false)
  assert.equal((await next).block, true)
})

test('a new message cancels a pending approval reached through call_tool, without ending the run', async () => {
  const permissions = botPermissions()
  const executed = []
  const tools = new Map([
    [
      'browser_automation',
      {
        name: 'browser_automation',
        parameters: {},
        execute: async (_id, args) => {
          executed.push(args)
          return { content: [] }
        },
      },
    ],
  ])
  const gateway = createToolGatewayTool({
    getTool: (name) => tools.get(name),
    authorize: (input) =>
      permissions.authorize({
        ...input,
        sessionId: 'conv',
        cwd: '/repo',
        noWait: (name) => botConversationApprovalRefusal({ toolName: name }),
      }),
  })
  const session = {
    agent: {
      runWithLifecycle: (run) => run(new AbortController().signal),
      streamFunction: async () => {},
      beforeToolCall: async () => undefined,
    },
  }
  installStepInterrupt(session)
  const run = new AbortController()
  await session.agent.beforeToolCall(
    { toolCall: { name: 'call_tool' }, context: { tools: [gateway] } },
    run.signal,
  )
  const call = gateway.execute(
    'call-1',
    { name: 'browser_automation', arguments: { action: 'click', selector: '#go' } },
    run.signal,
  )
  await waitForPending(permissions)
  assert.equal(interruptCurrentStep(session), true)
  await assert.rejects(call, /operation was stopped/)
  assert.deepEqual(permissions.getPending('conv'), [])
  assert.deepEqual(executed, [])
  assert.equal(run.signal.aborted, false)
})
