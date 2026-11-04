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
import {
  NO_WAIT_APPROVAL_REASON,
  SessionPermissionService,
} from '../services/session-permission-service.mjs'

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
