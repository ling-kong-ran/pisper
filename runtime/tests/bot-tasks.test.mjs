import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import { BotTaskService, botTaskPrompt } from '../services/bots/bot-task-service.mjs'
import { LocalExecutor } from '../services/bots/executor.mjs'
import { WorktreeManager } from '../services/bots/worktree-manager.mjs'

const IDENTITY = ['-c', 'user.name=Test', '-c', 'user.email=test@example.com']

function git(cwd, ...args) {
  return execFileSync('git', [...IDENTITY, ...args], { cwd, encoding: 'utf8' }).trim()
}

async function fixture(t, cleanups = []) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-tasks-'))
  // 先停服务（等它写完状态文件）再删目录。
  t.after(async () => {
    for (const cleanup of cleanups) await cleanup()
    await rm(directory, { recursive: true, force: true })
  })
  const repo = join(directory, 'repo')
  await mkdir(repo)
  git(repo, 'init', '-q', '-b', 'main')
  // 仓库里写好提交身份，Bot 提交与 cherry-pick 用这份配置。
  git(repo, 'config', 'user.name', 'Test')
  git(repo, 'config', 'user.email', 'test@example.com')
  await writeFile(join(repo, 'README.md'), 'hello\n')
  git(repo, 'add', '-A')
  git(repo, 'commit', '-q', '-m', 'init')
  const plain = join(directory, 'plain')
  await mkdir(plain)
  const worktrees = new WorktreeManager({
    executor: new LocalExecutor(),
    root: join(directory, 'data', 'worktrees'),
  })
  return { directory, repo, plain, worktrees }
}

function bot(id, overrides = {}) {
  return {
    id,
    ...normalizeBotInput({ name: id, instructions: '小心修改', workspace: { cwd: '/x' } }),
    ...overrides,
  }
}

function deferred() {
  let resolve
  let reject
  const promise = new Promise((ok, fail) => {
    resolve = ok
    reject = fail
  })
  return { promise, resolve, reject }
}

async function until(predicate, timeoutMs = 5_000) {
  const started = Date.now()
  while (!predicate()) {
    if (Date.now() - started > timeoutMs) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

test('worktrees isolate each task and accept applies through cherry-pick only', async (t) => {
  const { repo, plain, worktrees } = await fixture(t)
  await assert.rejects(worktrees.create({ cwd: plain, taskId: 'task_aaaaaaaa', botName: 'x' }), {
    code: 'workspace_not_git',
  })
  const ws = await worktrees.create({ cwd: repo, taskId: 'task_abcdefgh', botName: '审查员' })
  assert.equal(ws.branch, 'pisper/审查员/abcdefgh')
  assert.ok(ws.path.includes(join('data', 'worktrees')))
  assert.equal(git(repo, 'rev-parse', '--abbrev-ref', 'HEAD'), 'main')

  await writeFile(join(ws.path, 'README.md'), 'hello bot\n')
  await writeFile(join(ws.path, 'new.txt'), 'new\n')
  const changes = await worktrees.changes(ws)
  assert.deepEqual(changes.files.map((file) => [file.status, file.path]).sort(), [
    ['A', 'new.txt'],
    ['M', 'README.md'],
  ])
  assert.match(changes.diff, /\+hello bot/)
  // 用户的工作区没被动到。
  assert.equal(await readFile(join(repo, 'README.md'), 'utf8'), 'hello\n')

  const commit = await worktrees.commit(ws, '审查员: 改 README')
  assert.match(commit, /^[0-9a-f]{40}$/)
  await writeFile(join(repo, 'dirty.txt'), 'x')
  await assert.rejects(worktrees.apply(ws, commit), { code: 'repo_dirty' })
  await rm(join(repo, 'dirty.txt'))
  await worktrees.apply(ws, commit)
  assert.equal(await readFile(join(repo, 'README.md'), 'utf8'), 'hello bot\n')
  assert.equal(git(repo, 'log', '-1', '--format=%s'), '审查员: 改 README')

  await worktrees.remove(ws)
  assert.equal(existsSync(ws.path), false)
  assert.equal(git(repo, 'branch', '--list', ws.branch), `${ws.branch}`)
  await assert.rejects(worktrees.remove({ ...ws, state: 'ready', path: repo }), {
    code: 'worktree_outside_root',
  })
})

test('a conflicting apply is rolled back and leaves the branch for a human', async (t) => {
  const { repo, worktrees } = await fixture(t)
  const ws = await worktrees.create({ cwd: repo, taskId: 'task_conflict', botName: 'bot' })
  await writeFile(join(ws.path, 'README.md'), 'from bot\n')
  const commit = await worktrees.commit(ws, 'bot change')
  await writeFile(join(repo, 'README.md'), 'from human\n')
  git(repo, 'commit', '-q', '-am', 'human change')
  await assert.rejects(worktrees.apply(ws, commit), { code: 'apply_conflict' })
  assert.equal(git(repo, 'status', '--porcelain'), '')
  assert.equal(await readFile(join(repo, 'README.md'), 'utf8'), 'from human\n')
})

async function taskFixture(t, { bots, runAgent, maxRunning = 3, stored, waitingFor } = {}) {
  const cleanups = []
  const base = await fixture(t, cleanups)
  const registry = new Map(bots.map((item) => [item.id, item]))
  const path = join(base.directory, 'data', 'pisper-bot-tasks.json')
  if (stored) {
    await mkdir(join(base.directory, 'data'), { recursive: true })
    await writeFile(path, JSON.stringify(stored))
  }
  const stops = []
  const service = new BotTaskService({
    path,
    getBot: (id) => {
      const item = registry.get(id)
      return item
        ? {
            ...item,
            workspace: {
              ...item.workspace,
              cwd: item.workspace.cwd === '/x' ? base.repo : item.workspace.cwd,
            },
          }
        : null
    },
    worktrees: base.worktrees,
    runAgent,
    stopAgent: async (sessionId) => stops.push(sessionId),
    waitingFor,
    maxRunning,
  })
  cleanups.push(() => service.dispose())
  await service.init()
  return { ...base, service, stops, path }
}

test('a task with changes waits for review; accepting applies it and keeps the branch', async (t) => {
  const prompts = []
  const { service, repo } = await taskFixture(t, {
    bots: [bot('reviewer')],
    runAgent: async ({ cwd, message, onSession }) => {
      prompts.push(message)
      onSession('session-1')
      await writeFile(join(cwd, 'fix.txt'), 'fixed\n')
      return { text: '改好了', sessionId: 'session-1' }
    },
  })
  const task = await service.dispatch('reviewer', { text: '修一下 bug' })
  await until(() => service.get(task.id).status === 'review')
  const reviewed = service.get(task.id)
  assert.equal(reviewed.result.changedFiles, 1)
  assert.equal(reviewed.result.summary, '改好了')
  assert.equal(reviewed.sessionId, 'session-1')
  assert.match(prompts[0], /\[reviewer 的工作说明\]\n小心修改/)
  assert.match(prompts[0], /不要推送/)
  assert.match((await service.diff(task.id)).diff, /\+fixed/)
  assert.equal(service.isBusy('reviewer'), true)

  const accepted = await service.accept(task.id, { apply: true })
  assert.equal(accepted.status, 'done')
  assert.equal(accepted.result.applied, true)
  assert.equal(await readFile(join(repo, 'fix.txt'), 'utf8'), 'fixed\n')
  assert.equal(existsSync(accepted.workspace.path), false)
  assert.equal(git(repo, 'branch', '--list', accepted.workspace.branch), accepted.workspace.branch)
  assert.equal(service.isBusy('reviewer'), false)
})

test('no changes or reply-only delivery finish right away; discard removes the branch', async (t) => {
  let write = false
  const { service, repo } = await taskFixture(t, {
    bots: [bot('quiet'), bot('talker', { delivery: 'reply-only' })],
    runAgent: async ({ cwd }) => {
      if (write) await writeFile(join(cwd, 'x.txt'), 'x')
      return { text: 'ok' }
    },
  })
  const quiet = await service.dispatch('quiet', { text: '看看就好' })
  await until(() => service.get(quiet.id).status === 'done')
  assert.equal(git(repo, 'branch', '--list', service.get(quiet.id).workspace.branch), '')

  write = true
  const talker = await service.dispatch('talker', { text: '只回复' })
  await until(() => service.get(talker.id).status === 'done')
  assert.equal(service.get(talker.id).result.changedFiles, 0)

  const changed = await service.dispatch('quiet', { text: '改一下' })
  await until(() => service.get(changed.id).status === 'review')
  const discarded = await service.discard(changed.id)
  assert.equal(discarded.status, 'discarded')
  assert.equal(git(repo, 'branch', '--list', discarded.workspace.branch), '')
})

test('each bot runs one task at a time and the global limit is respected', async (t) => {
  const gates = new Map()
  const { service } = await taskFixture(t, {
    bots: ['a', 'b', 'c', 'd'].map((id) => bot(id, { delivery: 'reply-only' })),
    maxRunning: 3,
    runAgent: ({ task }) => {
      const gate = deferred()
      gates.set(task.id, gate)
      return gate.promise
    },
  })
  const a1 = await service.dispatch('a', { text: '1' })
  const a2 = await service.dispatch('a', { text: '2' })
  const b = await service.dispatch('b', { text: '3' })
  const c = await service.dispatch('c', { text: '4' })
  const d = await service.dispatch('d', { text: '5' })
  await until(() => gates.size === 3)
  const status = (task) => service.get(task.id).status
  assert.equal(status(a1), 'running')
  assert.equal(status(a2), 'queued')
  assert.equal(status(d), 'queued')
  gates.get(a1.id).resolve({ text: 'done' })
  // a1 结束后：a 的下一个任务与 d 竞争空位，按排队先后，a2 先上。
  // 状态先变成 running，代理才被调用；等到代理真的接手（gate 已登记）再往下走，满载时也不抢跑。
  await until(() => status(a1) === 'done' && status(a2) === 'running' && gates.has(a2.id))
  for (const task of [b, c]) gates.get(task.id).resolve({ text: 'ok' })
  await until(() => status(d) === 'running' && gates.has(d.id))
  for (const task of [a2, d]) gates.get(task.id).resolve({ text: 'ok' })
  await until(() => status(a2) === 'done' && status(d) === 'done')
})

test('stopping a running task aborts its session and marks it stopped', async (t) => {
  const gate = deferred()
  const { service, stops } = await taskFixture(t, {
    bots: [bot('slow')],
    runAgent: ({ onSession }) => {
      onSession('session-slow')
      return gate.promise
    },
  })
  const task = await service.dispatch('slow', { text: '跑很久' })
  await until(() => service.get(task.id).status === 'running' && service.get(task.id).sessionId)
  const stopping = service.stop(task.id)
  await until(() => stops.length === 1)
  gate.reject(new Error('aborted'))
  await stopping
  assert.deepEqual(stops, ['session-slow'])
  assert.equal(service.get(task.id).status, 'failed')
  assert.equal(service.get(task.id).error.code, 'stopped')
  // 失败的任务可以继续，沿用原来的 worktree。
  const resumed = await service.resume(task.id)
  // 继续会立即进入调度，返回时可能已在准备。
  assert.ok(['queued', 'preparing'].includes(resumed.status), resumed.status)
})

test('restart turns running tasks into interrupted and requeues preparing ones', async (t) => {
  const started = []
  const { service } = await taskFixture(t, {
    bots: [bot('a', { delivery: 'reply-only' }), bot('b', { delivery: 'reply-only' })],
    runAgent: async ({ task }) => {
      started.push(task.id)
      return { text: 'ok' }
    },
    stored: {
      version: 1,
      tasks: [
        {
          id: 'task_running',
          botId: 'a',
          botName: 'a',
          status: 'running',
          request: 'x',
          title: 'x',
        },
        {
          id: 'task_prep',
          botId: 'b',
          botName: 'b',
          status: 'preparing',
          request: 'y',
          title: 'y',
        },
        { id: 'broken' },
      ],
    },
  })
  assert.equal(service.get('task_running').status, 'interrupted')
  assert.equal(service.get('task_running').error.code, 'runtime_restarted')
  await until(() => service.get('task_prep').status === 'done')
  assert.deepEqual(started, ['task_prep'])
  assert.equal(service.get('broken'), null)
})

test('dispatch validates the bot and request; non-git workspaces fail clearly', async (t) => {
  const { service, plain } = await taskFixture(t, {
    bots: [bot('paused', { enabled: false }), bot('plain')],
    runAgent: async () => ({ text: 'ok' }),
  })
  await assert.rejects(service.dispatch('paused', { text: 'x' }), { code: 'bot_paused' })
  await assert.rejects(service.dispatch('missing', { text: 'x' }), { code: 'bot_not_found' })
  await assert.rejects(service.dispatch('plain', { text: '   ' }), { code: 'bot_task_empty' })
  // 指向非 git 目录、又要求独立分支的 Bot。
  const nonGit = await taskFixture(t, {
    bots: [
      bot('plain', { workspace: { environmentId: 'local', cwd: plain, isolation: 'worktree' } }),
    ],
    runAgent: async () => ({ text: 'ok' }),
  })
  const task = await nonGit.service.dispatch('plain', { text: '改代码' })
  await until(() => nonGit.service.get(task.id).status === 'failed')
  assert.equal(nonGit.service.get(task.id).error.code, 'workspace_not_git')
})

test('the task prompt keeps instructions and the request apart', () => {
  const prompt = botTaskPrompt(
    { name: '日报', instructions: '' },
    { request: '汇总今天的提交' },
    { isolated: false },
  )
  assert.equal(prompt, '[任务]\n汇总今天的提交')
})

test('task routes validate the status filter and go through the Labs gate', async () => {
  const calls = []
  const runtime = {
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => true,
    listBotTasks: (query) => {
      calls.push(query)
      return []
    },
  }
  const handler = createApiHandler(runtime)
  const respond = async (path) => {
    const output = {
      status: 0,
      body: '',
      destroyed: false,
      writableEnded: false,
      headersSent: false,
      writeHead(s) {
        this.status = s
      },
      flushHeaders() {},
      write(b = '') {
        this.body += b
      },
      end(b = '') {
        this.body += b
        this.writableEnded = true
      },
    }
    await handler(
      { method: 'GET', async *[Symbol.asyncIterator]() {} },
      output,
      new URL(`http://localhost${path}`),
    )
    return { status: output.status, body: JSON.parse(output.body) }
  }
  assert.equal((await respond('/api/bot-tasks?status=bogus')).body.code, 'bot_task_status_invalid')
  assert.equal((await respond('/api/bot-tasks?botId=b1&status=review&limit=5')).status, 200)
  assert.deepEqual(calls, [{ botId: 'b1', status: 'review', limit: 5 }])
})

test('talking to a bot while changes await review continues the same task on the same branch', async (t) => {
  const calls = []
  const { service } = await taskFixture(t, {
    bots: [bot('reviewer', { threadSessionId: 'thread-1' })],
    runAgent: async ({ cwd, sessionId, message }) => {
      calls.push({ cwd, sessionId, message })
      await writeFile(join(cwd, `turn-${calls.length}.txt`), 'x\n')
      return { text: `第 ${calls.length} 轮改好了`, sessionId: 'thread-1' }
    },
  })
  const first = await service.message('reviewer', { text: '修 bug' })
  await until(() => service.get(first.id).status === 'review')
  assert.equal(service.activity('reviewer').needsYou, true)

  const followUp = await service.message('reviewer', { text: '再补个测试' })
  assert.equal(followUp.id, first.id)
  await until(() => service.get(first.id).status === 'review' && calls.length === 2)
  const task = service.get(first.id)
  assert.deepEqual(
    task.turns.map((turn) => [turn.text, turn.reply]),
    [
      ['修 bug', '第 1 轮改好了'],
      ['再补个测试', '第 2 轮改好了'],
    ],
  )
  // 两轮都在 Bot 的长期会话、同一个 worktree 里跑；第二轮只带修改意见。
  assert.deepEqual(
    calls.map((call) => call.sessionId),
    ['thread-1', 'thread-1'],
  )
  assert.equal(calls[0].cwd, calls[1].cwd)
  assert.match(calls[1].message, /^\[接着改\]\n再补个测试/)
  assert.equal(task.result.changedFiles, 2)

  // 处理完以后再说话，是一件新的事。
  await service.discard(first.id)
  const next = await service.message('reviewer', { text: '看看 CI' })
  assert.notEqual(next.id, first.id)
})

test('a running task shows the approval or question it waits for without storing it', async (t) => {
  const gate = deferred()
  const pending = new Map()
  const { service, path } = await taskFixture(t, {
    bots: [bot('careful')],
    runAgent: ({ onSession }) => {
      onSession('session-careful')
      return gate.promise
    },
    waitingFor: (sessionId) => pending.get(sessionId) ?? null,
  })
  const task = await service.dispatch('careful', { text: '清理构建目录' })
  await until(() => service.get(task.id).status === 'running' && service.get(task.id).sessionId)
  assert.equal(service.get(task.id).waiting, undefined)
  assert.equal(service.activity('careful').needsYou, false)

  const approval = { id: 'a1', toolName: 'bash', argsPreview: 'rm -rf dist', severity: 'standard' }
  pending.set('session-careful', { approval, question: null })
  assert.deepEqual(service.get(task.id).waiting, { approval, question: null })
  assert.deepEqual(service.list({ botId: 'careful' })[0].waiting, { approval, question: null })
  assert.equal(service.activity('careful').needsYou, true)
  assert.equal(service.activity('careful').waiting, 'approval')

  pending.set('session-careful', { approval: null, question: { id: 'q1', prompt: '删哪个？' } })
  assert.equal(service.activity('careful').waiting, 'question')

  // 只在内存里投影，不写进任务文件。
  await service.save()
  assert.doesNotMatch(await readFile(path, 'utf8'), /waiting|rm -rf/)

  pending.clear()
  gate.resolve({ text: '清理好了', sessionId: 'session-careful' })
  await until(() => service.get(task.id).status !== 'running')
  assert.equal(service.activity('careful').waiting, null)
})

test('the runtime reports the oldest pending approval in board-card form and the question summary', () => {
  const approvals = [
    {
      id: 'late',
      toolName: 'bash',
      args: { command: 'ls' },
      createdAt: '2026-11-03T00:02:00.000Z',
    },
    {
      id: 'early',
      toolName: 'bash',
      args: { command: 'rm -rf /' },
      reason: '删除根目录',
      severity: 'critical',
      createdAt: '2026-11-03T00:01:00.000Z',
    },
  ]
  const questions = [
    {
      id: 'q1',
      sessionId: 's1',
      createdAt: '2026-11-03T00:03:00.000Z',
      questions: [
        { question: '删哪个？', header: '目录', multiSelect: false, options: [{ label: 'dist' }] },
      ],
    },
  ]
  const runtime = {
    permissions: { getPending: (id) => (id === 's1' ? approvals : []) },
    questions: { getPending: (id) => (id === 's1' ? questions : []) },
  }
  const waiting = AgentRuntimeFacade.prototype.botTaskWaiting.call(runtime, 's1')
  assert.equal(waiting.approval.id, 'early')
  assert.equal(waiting.approval.argsPreview, 'rm -rf /')
  assert.equal(waiting.approval.severity, 'critical')
  assert.equal(waiting.question.id, 'q1')
  assert.deepEqual(waiting.question.options, ['dist'])
  assert.deepEqual(AgentRuntimeFacade.prototype.botTaskWaiting.call(runtime, 'other'), {
    approval: null,
    question: null,
  })
})

test('bot runs pass the bot tool scope to the agent run', async () => {
  const calls = []
  const runtime = {
    providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    sessions: new Map(),
    findSessionInfo: async () => null,
    promptFromChannel: async (input) => {
      calls.push(input)
      return { sessionId: 'thread-1', text: 'ok' }
    },
    bots: { setThread: async () => {} },
  }
  const run = (tools) =>
    AgentRuntimeFacade.prototype.runBotTaskAgent.call(runtime, {
      bot: {
        id: 'b',
        name: '审查员',
        model: { tier: 'default' },
        executionMode: 'approval-required',
        tools,
      },
      task: { title: '看看' },
      cwd: '/repo',
      message: 'hi',
    })
  await run(['read', 'grep'])
  await run(null)
  assert.deepEqual(calls[0].allowedToolNames, ['read', 'grep'])
  assert.equal(calls[1].allowedToolNames, null)
  assert.equal(calls[0].isolatedContext, true)
})

test('a bot reply is what the agent said after its last tool call', async () => {
  // 中间每一步的旁白（“先看一下…”“跑一下测试”）不进 Bot 的回复；频道仍拿到完整文字。
  const runtime = {
    sessions: new Map(),
    pendingSessions: new Map(),
    sessionMeta: {},
    cwd: '/repo',
    assetIndex: { assets: [] },
    findSessionInfo: async () => ({ id: 's1' }),
    streamPrompt: async ({ send }) => {
      send('text_delta', { delta: '先看一下。' })
      send('tool_start', { name: 'read' })
      send('text_delta', { delta: '修好了，' })
      send('text_delta', { delta: '测试通过。' })
      send('done', { sessionId: 's1' })
    },
    getSessionMessages: async () => [],
  }
  const result = await AgentRuntimeFacade.prototype.promptFromChannel.call(runtime, {
    sessionId: 's1',
    message: 'hi',
  })
  assert.equal(result.text, '先看一下。修好了，测试通过。')
  assert.equal(result.finalText, '修好了，测试通过。')
  const bot = await AgentRuntimeFacade.prototype.runBotTaskAgent.call(
    {
      providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
      sessions: new Map(),
      findSessionInfo: async () => null,
      promptFromChannel: async () => result,
      bots: { setThread: async () => {} },
    },
    {
      bot: { id: 'b', name: '小橙', model: { tier: 'default' }, tools: null },
      task: { title: '修' },
      cwd: '/repo',
      message: 'hi',
    },
  )
  assert.equal(bot.text, '修好了，测试通过。')
})

test('a failed agent run fails the bot task instead of finishing it with no changes', async () => {
  // streamPrompt 出错时只广播 error 事件不抛出：promptFromChannel 要把它带回来，Bot 运行据此失败。
  const events = []
  const runtime = {
    sessions: new Map(),
    pendingSessions: new Map(),
    sessionMeta: {},
    cwd: '/repo',
    assetIndex: { assets: [] },
    findSessionInfo: async () => ({ id: 's1' }),
    streamPrompt: async ({ send }) => {
      send('meta', { sessionId: 's1' })
      send('error', { sessionId: 's1', message: 'Connection error.' })
    },
    getSessionMessages: async () => [],
  }
  const result = await AgentRuntimeFacade.prototype.promptFromChannel.call(runtime, {
    sessionId: 's1',
    message: 'hi',
    onEvent: (event) => events.push(event),
  })
  assert.equal(result.error, 'Connection error.')
  assert.deepEqual(events, ['meta', 'error'])

  const failing = {
    providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    sessions: new Map(),
    findSessionInfo: async () => null,
    promptFromChannel: async () => ({ sessionId: 's1', text: '', error: 'Connection error.' }),
    bots: { setThread: async () => {} },
  }
  await assert.rejects(
    AgentRuntimeFacade.prototype.runBotTaskAgent.call(failing, {
      bot: {
        id: 'b',
        name: '小橙',
        model: { tier: 'default' },
        executionMode: 'approval-required',
        tools: null,
      },
      task: { title: '修' },
      cwd: '/repo',
      message: 'hi',
    }),
    (error) => error.code === 'agent_run_failed' && error.message === 'Connection error.',
  )
})
