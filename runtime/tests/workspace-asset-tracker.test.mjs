import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { getEventListeners } from 'node:events'
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setImmediate as nextTick } from 'node:timers/promises'
import test from 'node:test'
import { promisify } from 'node:util'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { workspaceAssetFile } from '../services/workspace-asset-capture.mjs'
import {
  MAX_PENDING_FILES_PER_SESSION,
  MAX_SHELL_CAPTURED_FILES,
  WorkspaceAssetTracker,
} from '../services/workspace-asset-tracker.mjs'
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'

const execFileAsync = promisify(execFile)

const git = (cwd, ...args) =>
  execFileAsync(
    'git',
    ['-c', 'user.name=Pisper Test', '-c', 'user.email=test@example.invalid', ...args],
    { cwd },
  )

// 建一个带两个提交的仓库：main 上一个文件，update 分支改它并再加两个（模拟待更新的代码）。
async function repositoryWithUpdate(path) {
  await mkdir(path, { recursive: true })
  await git(path, 'init', '-q', '-b', 'main')
  await writeFile(join(path, 'tracked.txt'), 'v1')
  await git(path, 'add', '.')
  await git(path, 'commit', '-q', '-m', 'v1')
  await git(path, 'checkout', '-q', '-b', 'update')
  await writeFile(join(path, 'tracked.txt'), 'v2 is longer')
  await mkdir(join(path, 'docs'))
  await writeFile(join(path, 'docs', 'added.md'), 'added')
  await writeFile(join(path, 'docs', 'promo.mp4'), 'video')
  await git(path, 'add', '.')
  await git(path, 'commit', '-q', '-m', 'v2')
  await git(path, 'checkout', '-q', 'main')
}

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

async function waitingForLock(signal) {
  const deadline = Date.now() + 3_000
  while (!getEventListeners(signal, 'abort').length) {
    assert.ok(Date.now() < deadline, '调用应进入可取消的锁等待')
    await nextTick()
  }
}

async function fixture(t, archiveHook, trackerOptions = {}) {
  // macOS 的 TMPDIR 常为 /var 符号链接形态，而 tracker 会以 realpath 规范化 cwd；
  // fixture 根目录统一为真实路径，断言路径才能与归档输出在任意 shell 环境下一致。
  const root = await realpath(await mkdtemp(join(tmpdir(), 'pisper-asset-tracker-')))
  const cwd = join(root, 'workspace')
  const dataDir = join(root, 'data')
  await Promise.all([mkdir(cwd), mkdir(dataDir)])
  t.after(() => rm(root, { recursive: true, force: true }))
  const archived = []
  const attempts = []
  const warnings = []
  const archive = async (sessionId, path, { hiddenInReply } = {}) => {
    const content = await readFile(path, 'utf8')
    const asset = { id: `${sessionId}:${path}`, sessionId, path, content, hiddenInReply }
    attempts.push(asset)
    await archiveHook?.(asset, attempts.length)
    archived.push(asset)
    return asset
  }
  const createTracker = (trackerDataDir = dataDir) =>
    new WorkspaceAssetTracker({
      dataDir: trackerDataDir,
      archive,
      warn: (error) => warnings.push(error),
      ...trackerOptions,
    })
  const tracker = createTracker()
  const run = (sessionId, execute, options = {}) =>
    tracker.run({ sessionId, cwd, name: 'bash', args: { command: 'fixture' }, ...options }, execute)
  return { cwd, dataDir, tracker, createTracker, run, archived, attempts, warnings }
}

async function directoryAlias(t, target, path) {
  try {
    // Windows 的目录 junction 不依赖管理员权限，其他平台使用目录符号链接。
    await symlink(target, path, process.platform === 'win32' ? 'junction' : 'dir')
    return true
  } catch (error) {
    if (!['EPERM', 'EACCES', 'ENOSYS', 'ENOTSUP', 'EOPNOTSUPP'].includes(error.code)) throw error
    t.skip(`当前环境无法创建目录链接：${error.code} ${error.message}`)
    return false
  }
}

function dispatch(session, tool, args, signal) {
  return (async () => {
    const decision = await session.agent.beforeToolCall?.(
      { toolCall: { id: 'fixture-call', name: tool.name }, args, context: { tools: [tool] } },
      signal,
    )
    if (decision?.block || signal?.aborted) return decision
    return tool.execute('fixture-call', args, signal)
  })()
}

test('concurrent shell calls in the same cwd archive only their own session files', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const controller = new AbortController()
  const first = f.run('session-a', async () => {
    await writeFile(join(f.cwd, 'a.txt'), 'from a')
    started.resolve()
    await release.promise
    return 'a-result'
  })
  await started.promise
  let secondExecuted = false
  const second = f.run(
    'session-b',
    async () => {
      secondExecuted = true
      await writeFile(join(f.cwd, 'b.txt'), 'from b')
      return 'b-result'
    },
    { signal: controller.signal },
  )
  try {
    await waitingForLock(controller.signal)
    assert.equal(secondExecuted, false)
  } finally {
    release.resolve()
  }
  assert.deepEqual(await Promise.all([first, second]), ['a-result', 'b-result'])
  const a = await f.tracker.drain('session-a')
  const b = await f.tracker.drain('session-b')
  assert.deepEqual(
    a.map(({ path, content }) => [path, content]),
    [[join(f.cwd, 'a.txt'), 'from a']],
  )
  assert.deepEqual(
    b.map(({ path, content }) => [path, content]),
    [[join(f.cwd, 'b.txt'), 'from b']],
  )
  assert.equal(f.attempts.length, 2)
  assert.deepEqual(await f.tracker.drain('session-a'), [])
})

test('a long-running shell yields the workspace without claiming files of later calls', async (t) => {
  const f = await fixture(t, undefined, { shellExclusiveMs: 20 })
  const started = deferred()
  const release = deferred()
  // 模拟 test:watch：先写一份产物，然后一直不退出。
  const watcher = f.run('watcher', async () => {
    await writeFile(join(f.cwd, 'watch-report.txt'), 'watch output')
    started.resolve()
    await release.promise
    await writeFile(join(f.cwd, 'watch-final.txt'), 'watch final')
    return 'watch-result'
  })
  await started.promise
  try {
    await f.run('shell', () => writeFile(join(f.cwd, 'typecheck.txt'), 'typecheck output'))
    await f.run('writer', () => writeFile(join(f.cwd, 'notes.txt'), 'notes'), {
      name: 'write',
      args: { path: 'notes.txt' },
    })
  } finally {
    release.resolve()
  }
  assert.equal(await watcher, 'watch-result')
  const contents = async (sessionId) =>
    (await f.tracker.drain(sessionId)).map((asset) => asset.content).sort()
  assert.deepEqual(await contents('shell'), ['typecheck output'])
  assert.deepEqual(await contents('writer'), ['notes'])
  assert.deepEqual(await contents('watcher'), ['watch final', 'watch output'])
  assert.equal(f.attempts.length, 4)
  assert.equal(f.tracker.detached.size, 0)
})

test('shell overwrites stay archived but are not delivered to the reply again', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, 'report.txt')
  await f.run('session-a', () => writeFile(path, 'first'))
  assert.deepEqual(
    (await f.tracker.drain('session-a')).map((asset) => asset.content),
    ['first'],
  )
  await f.run('session-a', () => writeFile(path, 'second version'))
  assert.deepEqual(await f.tracker.drain('session-a'), [])
  assert.deepEqual(
    f.archived.map(({ path: assetPath, content, hiddenInReply }) => [
      assetPath,
      content,
      hiddenInReply,
    ]),
    [
      [path, 'first', false],
      [path, 'second version', true],
    ],
  )
})

test('empty turns and read tools do not claim external files', async (t) => {
  const f = await fixture(t)
  f.tracker.install({ agent: {} }, { sessionId: 'empty', cwd: f.cwd })
  await writeFile(join(f.cwd, 'outside-turn.txt'), 'external')
  assert.deepEqual(await f.tracker.drain('empty'), [])
  await f.run(
    'reader',
    async () => {
      // 模拟只读工具执行期间由工作区外部写入的文件。
      await writeFile(join(f.cwd, 'outside-read.txt'), 'external during read')
      return readFile(join(f.cwd, 'outside-turn.txt'), 'utf8')
    },
    { name: 'read', args: { path: 'outside-turn.txt' } },
  )
  assert.deepEqual(await f.tracker.drain('reader'), [])
  await f.run('readonly-shell', () => readFile(join(f.cwd, 'outside-turn.txt'), 'utf8'))
  assert.deepEqual(await f.tracker.drain('readonly-shell'), [])
  assert.deepEqual(f.attempts, [])
})

test('one shell call archives all 201 generated files', async (t) => {
  const f = await fixture(t)
  const expected = Array.from({ length: 201 }, (_, index) => [
    join(f.cwd, `report-${String(index).padStart(3, '0')}.txt`),
    `report ${index}`,
  ])
  await f.run('batch', () =>
    Promise.all(expected.map(([path, content]) => writeFile(path, content))),
  )
  const assets = await f.tracker.drain('batch')
  assert.equal(assets.length, 201)
  assert.deepEqual(
    assets.map(({ path, content }) => [path, content]).sort(([a], [b]) => a.localeCompare(b)),
    expected,
  )
  assert.equal(f.attempts.length, 201)
  assert.deepEqual(f.warnings, [])
})

test('updating code through git is archived but hidden from the reply', async (t) => {
  const f = await fixture(t)
  await repositoryWithUpdate(f.cwd)
  await f.run('pull', () => git(f.cwd, 'merge', '-q', '--ff-only', 'update'))
  assert.deepEqual(await f.tracker.drain('pull'), [])
  assert.deepEqual(
    f.archived.map((asset) => asset.hiddenInReply),
    [true, true, true],
  )
})

test('a clone into the workspace is archived but hidden from the reply', async (t) => {
  const f = await fixture(t)
  const source = join(f.cwd, '..', 'source')
  await repositoryWithUpdate(source)
  await f.run('clone', () => git(f.cwd, 'clone', '-q', source, 'cloned'))
  assert.deepEqual(await f.tracker.drain('clone'), [])
  assert.equal(f.archived.length, 1)
  assert.equal(f.archived[0].hiddenInReply, true)
})

test('only files the agent created reach the reply next to a git update', async (t) => {
  const f = await fixture(t)
  const repository = join(f.cwd, 'repository')
  // 工作区是仓库的子目录时，也要认出上层仓库。
  const cwd = join(repository, 'docs')
  await repositoryWithUpdate(repository)
  await mkdir(cwd)
  await f.run(
    'mixed',
    async () => {
      await git(repository, 'merge', '-q', '--ff-only', 'update')
      await writeFile(join(cwd, 'report.txt'), 'untracked output')
      await writeFile(join(cwd, 'added.md'), 'tracked file changed by a script')
    },
    { cwd },
  )
  assert.deepEqual(
    (await f.tracker.drain('mixed')).map((asset) => asset.content),
    ['untracked output'],
  )
  assert.equal(f.archived.length, 3)
})

test('files the agent creates and then stages or commits still reach the reply', async (t) => {
  const f = await fixture(t)
  await repositoryWithUpdate(f.cwd)
  await f.run('staged', async () => {
    await writeFile(join(f.cwd, 'staged.txt'), 'staged output')
    await git(f.cwd, 'add', 'staged.txt')
  })
  assert.deepEqual(
    (await f.tracker.drain('staged')).map((asset) => asset.content),
    ['staged output'],
  )
  await f.run('committed', async () => {
    await writeFile(join(f.cwd, 'committed.txt'), 'committed output')
    await git(f.cwd, 'add', 'committed.txt')
    await git(f.cwd, 'commit', '-q', '-m', 'add output')
  })
  assert.deepEqual(
    (await f.tracker.drain('committed')).map((asset) => asset.content),
    ['committed output'],
  )
})

test('a repository without commits does not hide staged files', async (t) => {
  const f = await fixture(t)
  await git(f.cwd, 'init', '-q')
  await f.run('fresh', async () => {
    await writeFile(join(f.cwd, 'first.txt'), 'first output')
    await git(f.cwd, 'add', 'first.txt')
  })
  assert.deepEqual(
    (await f.tracker.drain('fresh')).map((asset) => asset.content),
    ['first output'],
  )
})

test('a merge stopped by a conflict does not attribute incoming files to the agent', async (t) => {
  const f = await fixture(t)
  await repositoryWithUpdate(f.cwd)
  await writeFile(join(f.cwd, 'tracked.txt'), 'conflicting change')
  await git(f.cwd, 'commit', '-q', '-am', 'local change')
  await f.run('merge', () =>
    assert.rejects(git(f.cwd, 'merge', '-q', '--no-edit', '--no-ff', 'update')),
  )
  await assert.doesNotReject(readFile(join(f.cwd, 'docs', 'added.md')))
  assert.deepEqual(await f.tracker.drain('merge'), [])
  assert.ok(f.archived.length > 0)
})

test('restoring deleted tracked files is not attributed to the agent', async (t) => {
  const f = await fixture(t)
  await repositoryWithUpdate(f.cwd)
  await rm(join(f.cwd, 'tracked.txt'))
  await f.run('restore', () => git(f.cwd, 'checkout', '--', 'tracked.txt'))
  assert.deepEqual(await f.tracker.drain('restore'), [])
})

test('a broken git repository does not hide created files', async (t) => {
  const f = await fixture(t)
  await mkdir(join(f.cwd, '.git'))
  await f.run('broken', () => writeFile(join(f.cwd, 'report.txt'), 'output'))
  assert.deepEqual(
    (await f.tracker.drain('broken')).map((asset) => asset.content),
    ['output'],
  )
})

test('restored retries from before the upgrade keep showing in the reply', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, 'legacy.txt')
  await writeFile(path, 'legacy')
  const file = await workspaceAssetFile(path)
  await writeFile(
    join(f.dataDir, 'pisper-workspace-asset-retries.json'),
    JSON.stringify({ files: [{ ...file, sessionId: 'legacy', cwd: f.cwd }] }),
  )
  assert.deepEqual(
    (await f.createTracker().drain('legacy')).map((asset) => asset.content),
    ['legacy'],
  )
})

test('a real agent turn shows only what the agent produced, live and after a reload', async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'pisper-asset-reply-')))
  const directory = join(root, 'data')
  const workspace = join(root, 'workspace')
  await mkdir(directory)
  await repositoryWithUpdate(workspace)
  const chunk = (delta, finish_reason = null) => ({
    id: 'test',
    object: 'chat.completion.chunk',
    created: 0,
    model: 'fixture',
    choices: [{ index: 0, delta, finish_reason }],
  })
  const toolCalls = {
    更新代码: {
      name: 'bash',
      arguments: { command: 'git merge -q --ff-only update && echo output > report.txt' },
    },
    新建文件: { name: 'write', arguments: { path: 'notes.md', content: 'notes' } },
    改文件: {
      name: 'edit',
      arguments: { path: 'tracked.txt', edits: [{ oldText: 'v2 is longer', newText: 'v3' }] },
    },
  }
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const parts = []
    for await (const part of request) parts.push(part)
    const last = JSON.parse(Buffer.concat(parts).toString('utf8')).messages.at(-1)
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = (value) => reply.write(`data: ${JSON.stringify(value)}\n\n`)
    if (last.role === 'tool') {
      send(chunk({ role: 'assistant', content: 'done' }))
      send(chunk({}, 'stop'))
    } else {
      const text = JSON.stringify(last.content)
      const key = Object.keys(toolCalls).find((name) => text.includes(name))
      const { name, arguments: toolArguments } = toolCalls[key]
      send(
        chunk({
          role: 'assistant',
          tool_calls: [
            {
              index: 0,
              id: `call_${name}`,
              type: 'function',
              function: { name, arguments: JSON.stringify(toolArguments) },
            },
          ],
        }),
      )
      send(chunk({}, 'tool_calls'))
    }
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(root, { recursive: true, force: true })
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'test-reply': {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
          models: [
            {
              id: 'fixture',
              name: 'Fixture',
              reasoning: false,
              input: ['text'],
              contextWindow: 200000,
              maxTokens: 8192,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'test-reply': { type: 'api_key', key: 'test-only-reply-key' } }),
  )
  const start = async () => {
    const service = new AgentRuntimeService({ cwd: workspace, dataDir: directory })
    service.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
    await service.init()
    service.memory.setSemanticSummarizer(null)
    return service
  }
  runtime = await start()
  const created = await runtime.createSession('产物', workspace)
  await runtime.setSessionModel(created.id, 'test-reply', 'fixture')
  await runtime.setSessionExecutionMode(created.id, 'full-access')
  const turn = async (message) => {
    const names = { streamed: [], done: [] }
    await runtime.streamPrompt({
      sessionId: created.id,
      message,
      send: (event, data) => {
        if (event === 'generated_asset') names.streamed.push(data.name)
        if (event === 'done') names.done.push(...(data.assets || []).map((asset) => asset.name))
      },
    })
    return names
  }
  const attachedNames = async () =>
    (await runtime.getSessionMessages(created.id))
      .flatMap((message) => (message.attachments || []).map((attachment) => attachment.name))
      .sort()
  const namesPerReply = async () =>
    (await runtime.getSessionMessages(created.id))
      .filter((message) => message.role === 'agent')
      .map((message) => (message.attachments || []).map((attachment) => attachment.name))

  // bash 更新代码并生成一个文件：只有生成的文件进回复。
  assert.deepEqual(await turn('更新代码'), { streamed: ['report.txt'], done: ['report.txt'] })
  assert.equal(await readFile(join(workspace, 'docs', 'added.md'), 'utf8'), 'added')
  assert.deepEqual(await attachedNames(), ['report.txt'])

  // write 新建的文件进回复。
  assert.deepEqual(await turn('新建文件'), { streamed: ['notes.md'], done: ['notes.md'] })
  assert.deepEqual(await attachedNames(), ['notes.md', 'report.txt'])
  // 从历史重建时，产物要挂回产生它的那一轮，而不是上一条回复。
  assert.deepEqual(await namesPerReply(), [['report.txt'], ['notes.md']])

  // 删掉后重建的文件挂到新的一轮，之前那轮保持原样；刷新后也一样。
  await rm(join(workspace, 'notes.md'))
  assert.deepEqual(await turn('新建文件'), { streamed: ['notes.md'], done: ['notes.md'] })
  assert.deepEqual(await namesPerReply(), [['report.txt'], ['notes.md'], ['notes.md']])

  // edit 改已有文件不进回复，但仍能在“改动”面板里查看并撤销。
  assert.deepEqual(await turn('改文件'), { streamed: [], done: [] })
  assert.equal(await readFile(join(workspace, 'tracked.txt'), 'utf8'), 'v3')
  assert.deepEqual(await attachedNames(), ['notes.md', 'notes.md', 'report.txt'])
  const changes = await runtime.getSessionFileChanges(created.id)
  assert.equal(changes.files.find((file) => file.path === 'tracked.txt')?.canRevert, true)
  assert.equal((await runtime.revertSessionFileChanges(created.id, 'tracked.txt')).reverted, 1)
  assert.equal(await readFile(join(workspace, 'tracked.txt'), 'utf8'), 'v2 is longer')

  // generate_visual 等生成工具走默认入口，照常进回复。
  const chart = join(root, 'chart.png')
  await writeFile(chart, 'image')
  await runtime.recordGeneratedFile(created.id, { name: '产物' }, chart)
  assert.deepEqual(await attachedNames(), ['chart.png', 'notes.md', 'notes.md', 'report.txt'])

  // 重开运行时等同于刷新后从磁盘重建：隐藏标记必须落盘，资产页则保留全部文件。
  await runtime.dispose()
  runtime = await start()
  assert.deepEqual(await attachedNames(), ['chart.png', 'notes.md', 'notes.md', 'report.txt'])
  assert.deepEqual(
    (await runtime.listAssets({ sessionId: created.id })).map((a) => a.name).sort(),
    ['added.md', 'chart.png', 'notes.md', 'promo.mp4', 'report.txt', 'tracked.txt'],
  )
})

test('drain retries a failed archive once and consumes successful deliveries', async (t) => {
  const failure = new Error('archive unavailable')
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw failure
  })
  await f.run('retry', () => writeFile(join(f.cwd, 'retry.txt'), 'retry content'))
  assert.deepEqual(f.archived, [])
  assert.deepEqual(f.warnings, [failure])
  const assets = await f.tracker.drain('retry')
  assert.equal(assets.length, 1)
  assert.equal(assets[0].content, 'retry content')
  assert.equal(f.attempts.length, 2)
  assert.deepEqual(await f.tracker.drain('retry'), [])
  assert.equal(f.attempts.length, 2)
})

test('a new tracker restores failed archives from dataDir and persists their completion', async (t) => {
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw new Error('temporary archive failure')
  })
  const path = join(f.cwd, 'persisted.txt')
  await f.run('restored', () => writeFile(path, 'persisted content'))
  const restored = f.createTracker()
  assert.deepEqual(await restored.drain('other-session'), [])
  assert.equal(f.attempts.length, 1)
  assert.deepEqual(
    (await restored.drain('restored')).map(({ sessionId, path: assetPath, content }) => [
      sessionId,
      assetPath,
      content,
    ]),
    [['restored', path, 'persisted content']],
  )
  assert.deepEqual(await f.createTracker().drain('restored'), [])
  assert.equal(f.attempts.length, 2)
})

test('retry never attributes a later session overwrite to the original session', async (t) => {
  const f = await fixture(t, (asset) => {
    if (asset.sessionId === 'old') throw new Error('old session archive failed')
  })
  const path = join(f.cwd, 'shared.txt')
  await f.run('old', () => writeFile(path, 'old content'))
  await f.run('new', () => writeFile(path, 'new content is longer'))
  assert.deepEqual(await f.tracker.drain('old'), [])
  assert.deepEqual(await f.tracker.drain('new'), [])
  assert.deepEqual(
    f.attempts.map(({ sessionId, content }) => [sessionId, content]),
    [
      ['old', 'old content'],
      ['new', 'new content is longer'],
    ],
  )
  assert.deepEqual(await f.createTracker().drain('old'), [])
  assert.equal(f.attempts.length, 2)
})

test('restored retries discard externally replaced files without claiming their new content', async (t) => {
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw new Error('archive failed before restart')
  })
  const path = join(f.cwd, 'external-overwrite.txt')
  await f.run('old', () => writeFile(path, 'old'))
  await writeFile(path, 'external replacement')
  assert.deepEqual(await f.createTracker().drain('old'), [])
  assert.equal(f.attempts.length, 1)
  assert.deepEqual(f.archived, [])
  assert.deepEqual(await f.createTracker().drain('old'), [])
})

test('explicit writes deliver new files and edits only archive, including outside cwd', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, '..', 'explicit.txt')
  await f.run('writer', () => writeFile(path, 'written'), {
    name: 'write',
    args: { path, content: 'written' },
  })
  await f.run('editor', () => writeFile(path, 'edited content'), {
    name: 'edit',
    args: { path, edits: [{ oldText: 'written', newText: 'edited content' }] },
  })
  assert.deepEqual(
    (await f.tracker.drain('writer')).map((asset) => asset.content),
    ['written'],
  )
  assert.deepEqual(await f.tracker.drain('editor'), [])
  assert.deepEqual(
    f.archived.map((asset) => [asset.path, asset.hiddenInReply]),
    [
      [path, false],
      [path, true],
    ],
  )
})

test('an explicit no-op edit does not claim unrelated external files', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, 'existing.txt')
  await writeFile(path, 'unchanged')
  const result = await f.run(
    'editor',
    async () => {
      await writeFile(join(f.cwd, 'external.txt'), 'external')
      return 'no matching edit'
    },
    { name: 'edit', args: { path: 'existing.txt', edits: [] } },
  )
  assert.equal(result, 'no matching edit')
  assert.deepEqual(await f.tracker.drain('editor'), [])
  assert.deepEqual(f.attempts, [])
})

test('install wraps call_tool routed to bash while preserving execution arguments and result', async (t) => {
  const f = await fixture(t)
  const signal = new AbortController().signal
  const args = { name: 'bash', arguments: { command: 'generate report' } }
  const result = { content: [{ type: 'text', text: 'written' }] }
  let approvals = 0
  let executions = 0
  const session = {
    agent: {
      beforeToolCall: async () => {
        approvals += 1
      },
    },
  }
  const tool = {
    name: 'call_tool',
    async execute(id, actualArgs, actualSignal) {
      executions += 1
      assert.equal(this, tool)
      assert.equal(id, 'fixture-call')
      assert.equal(actualArgs, args)
      assert.equal(actualSignal, signal)
      await writeFile(join(f.cwd, 'routed.txt'), 'routed bash content')
      return result
    },
  }
  f.tracker.install(session, { sessionId: 'routed', cwd: f.cwd })
  f.tracker.install(session, { sessionId: 'routed', cwd: f.cwd })
  assert.equal(await dispatch(session, tool, args, signal), result)
  assert.equal(approvals, 1)
  assert.equal(executions, 1)
  assert.deepEqual(
    (await f.tracker.drain('routed')).map((asset) => asset.content),
    ['routed bash content'],
  )
  assert.equal(f.attempts.length, 1)
})

test('blocked calls neither execute nor capture external files, even after an allowed call', async (t) => {
  const f = await fixture(t)
  const denied = { block: true, reason: 'not approved' }
  let decision = denied
  let executions = 0
  const session = { agent: { beforeToolCall: async () => decision } }
  const tool = {
    name: 'bash',
    async execute() {
      executions += 1
      await writeFile(join(f.cwd, 'allowed.txt'), 'approved content')
    },
  }
  const original = tool.execute
  f.tracker.install(session, { sessionId: 'approval', cwd: f.cwd })
  await writeFile(join(f.cwd, 'external.txt'), 'external')
  assert.equal(await dispatch(session, tool, { command: 'denied' }), denied)
  assert.equal(tool.execute, original)
  assert.equal(executions, 0)
  assert.deepEqual(await f.tracker.drain('approval'), [])
  decision = undefined
  await dispatch(session, tool, { command: 'allowed' })
  assert.equal((await f.tracker.drain('approval')).length, 1)
  decision = denied
  await writeFile(join(f.cwd, 'external-later.txt'), 'external later')
  assert.equal(await dispatch(session, tool, { command: 'denied again' }), denied)
  assert.equal(executions, 1)
  assert.deepEqual(await f.tracker.drain('approval'), [])
  assert.equal(f.attempts.length, 1)
})

test('aborting a lock waiter prevents execution and allows later calls to acquire the lock', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const first = f.run('holder', async () => {
    started.resolve()
    await release.promise
    await writeFile(join(f.cwd, 'holder.txt'), 'holder')
  })
  await started.promise
  const controller = new AbortController()
  const reason = new Error('cancelled while waiting')
  let executed = false
  const waiting = f.run(
    'cancelled',
    async () => {
      executed = true
    },
    { signal: controller.signal },
  )
  const rejected = assert.rejects(waiting, (error) => error === reason)
  try {
    await waitingForLock(controller.signal)
    controller.abort(reason)
    await rejected
    assert.equal(executed, false)
    assert.equal(getEventListeners(controller.signal, 'abort').length, 0)
  } finally {
    release.resolve()
  }
  await first
  await f.run('following', () => writeFile(join(f.cwd, 'following.txt'), 'following'))
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  assert.deepEqual(
    (await f.tracker.drain('following')).map((asset) => asset.content),
    ['following'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.sessionId),
    ['holder', 'following'],
  )
})

test('an already aborted file call never executes or captures files', async (t) => {
  const f = await fixture(t)
  const controller = new AbortController()
  const reason = new Error('cancelled before execution')
  controller.abort(reason)
  let executed = false
  await assert.rejects(
    f.run(
      'cancelled',
      async () => {
        executed = true
      },
      { signal: controller.signal },
    ),
    (error) => error === reason,
  )
  assert.equal(executed, false)
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  assert.deepEqual(f.attempts, [])
})

test('gateway names with whitespace still acquire the workspace lock and capture their output', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const controller = new AbortController()
  let executions = 0
  const authorizedNames = []
  const gateway = createToolGatewayTool({
    getTool: (name) =>
      name === 'bash'
        ? {
            parameters: { type: 'object' },
            async execute() {
              executions += 1
              await writeFile(join(f.cwd, 'gateway.txt'), 'gateway output')
              return { content: [{ type: 'text', text: 'gateway result' }] }
            },
          }
        : null,
    authorize: async ({ toolName }) => {
      authorizedNames.push(toolName)
    },
  })
  const session = { agent: {} }
  f.tracker.install(session, { sessionId: 'gateway', cwd: f.cwd })
  const holder = f.run('holder', async () => {
    await writeFile(join(f.cwd, 'holder.txt'), 'holder output')
    started.resolve()
    await release.promise
  })
  await started.promise
  const routed = dispatch(
    session,
    gateway,
    { name: ' \tbash\n ', arguments: { command: 'generate report' } },
    controller.signal,
  )
  try {
    await waitingForLock(controller.signal)
    assert.equal(executions, 0)
  } finally {
    release.resolve()
    await Promise.allSettled([holder, routed])
  }
  assert.equal((await routed).details.gatewayToolName, 'bash')
  assert.equal(executions, 1)
  assert.deepEqual(authorizedNames, ['bash'])
  assert.deepEqual(
    (await f.tracker.drain('holder')).map((asset) => asset.content),
    ['holder output'],
  )
  assert.deepEqual(
    (await f.tracker.drain('gateway')).map((asset) => asset.content),
    ['gateway output'],
  )
  assert.equal(f.attempts.length, 2)
})

test('aborting while ready is pending prevents execution and releases the acquired lock', async (t) => {
  const f = await fixture(t)
  const originalReady = f.tracker.ready
  await originalReady
  const enteredReady = deferred()
  const releaseReady = deferred()
  const controller = new AbortController()
  const reason = new Error('cancelled during capture preparation')
  // 等待真正进入 ready 后才取消，避免把此回归误测成取得锁之前的取消。
  f.tracker.ready = {
    then(onFulfilled, onRejected) {
      enteredReady.resolve()
      return releaseReady.promise.then(onFulfilled, onRejected)
    },
  }
  let executions = 0
  const cancelledPath = join(f.cwd, 'cancelled.txt')
  const running = f.run(
    'cancelled',
    async () => {
      executions += 1
      await writeFile(cancelledPath, 'must not execute')
    },
    { signal: controller.signal },
  )
  const rejected = assert.rejects(running, (error) => error === reason)
  try {
    await enteredReady.promise
    controller.abort(reason)
    releaseReady.resolve()
    await rejected
  } finally {
    releaseReady.resolve()
    await Promise.allSettled([running, rejected])
    f.tracker.ready = originalReady
  }
  assert.equal(executions, 0)
  await assert.rejects(readFile(cancelledPath), { code: 'ENOENT' })
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  await f.run('following', () => writeFile(join(f.cwd, 'following.txt'), 'following output'))
  assert.deepEqual(
    (await f.tracker.drain('following')).map((asset) => asset.content),
    ['following output'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.sessionId),
    ['following'],
  )
})

test('a cwd directory alias pointing at dataDir does not capture runtime files', async (t) => {
  const f = await fixture(t)
  const alias = join(f.cwd, '..', 'runtime-cwd-alias')
  if (!(await directoryAlias(t, f.dataDir, alias))) return
  await f.run(
    'internal-shell',
    () => writeFile(join(alias, 'runtime-state.json'), '{"internal":true}'),
    { cwd: alias },
  )
  assert.equal(await readFile(join(f.dataDir, 'runtime-state.json'), 'utf8'), '{"internal":true}')
  assert.deepEqual(await f.tracker.drain('internal-shell'), [])
  assert.deepEqual(f.attempts, [])
  assert.deepEqual(f.warnings, [])
})

test('explicit writes through an intermediate alias exclude dataDir configured by another alias', async (t) => {
  const f = await fixture(t)
  const configuredAlias = join(f.cwd, '..', 'configured-data-alias')
  const outputAlias = join(f.cwd, 'output-alias')
  if (!(await directoryAlias(t, f.dataDir, configuredAlias))) return
  if (!(await directoryAlias(t, f.dataDir, outputAlias))) return
  await mkdir(join(f.dataDir, 'nested'))
  const tracker = f.createTracker(configuredAlias)
  const path = join(outputAlias, 'nested', 'runtime-state.json')
  await tracker.run(
    { sessionId: 'internal-write', cwd: f.cwd, name: 'write', args: { path } },
    () => writeFile(path, '{"version":1}'),
  )
  await tracker.run({ sessionId: 'internal-edit', cwd: f.cwd, name: 'edit', args: { path } }, () =>
    writeFile(path, '{"version":2,"updated":true}'),
  )
  assert.equal(
    await readFile(join(f.dataDir, 'nested', 'runtime-state.json'), 'utf8'),
    '{"version":2,"updated":true}',
  )
  assert.deepEqual(await tracker.drain('internal-write'), [])
  assert.deepEqual(await tracker.drain('internal-edit'), [])
  assert.deepEqual(f.attempts, [])
  assert.deepEqual(f.warnings, [])
})

test('a failed shell still archives its output and releases the workspace lock', async (t) => {
  const f = await fixture(t)
  const failure = new Error('shell exited with status 1')
  await assert.rejects(
    f.run('failed-shell', async () => {
      await writeFile(join(f.cwd, 'partial.txt'), 'partial output')
      throw failure
    }),
    (error) => error === failure,
  )
  assert.deepEqual(
    (await f.tracker.drain('failed-shell')).map((asset) => asset.content),
    ['partial output'],
  )
  await f.run('next-shell', () => writeFile(join(f.cwd, 'next.txt'), 'next output'))
  assert.deepEqual(
    (await f.tracker.drain('next-shell')).map((asset) => asset.content),
    ['next output'],
  )
})

test('a shell command that rewrites a whole tree is a build, not a reply attachment', async (t) => {
  const f = await fixture(t)
  // 例如每次构建删掉重建几千个 protobuf 生成的 Java 类：整批不记，也不复制进资产库。
  const generated = join(f.cwd, 'src', 'generated')
  await mkdir(generated, { recursive: true })
  assert.equal(
    await f.run('session-a', async () => {
      for (let i = 0; i <= MAX_SHELL_CAPTURED_FILES; i += 1)
        await writeFile(join(generated, `Message${i}.java`), `class Message${i} {}`)
      return 'built'
    }),
    'built',
  )
  assert.deepEqual(await f.tracker.drain('session-a'), [])
  assert.equal(f.attempts.length, 0)
  // 同一工作区里普通的少量产物照常记录。
  await f.run('session-a', () => writeFile(join(f.cwd, 'report.md'), 'done'))
  assert.deepEqual(
    (await f.tracker.drain('session-a')).map(({ path }) => path),
    [join(f.cwd, 'report.md')],
  )
})

test('retries left over from an earlier bulk capture are dropped instead of replayed', async (t) => {
  const f = await fixture(t)
  const kept = join(f.cwd, 'kept.md')
  await writeFile(kept, 'kept')
  const keptFile = await workspaceAssetFile(kept)
  const bulk = Array.from({ length: MAX_PENDING_FILES_PER_SESSION + 1 }, (_, i) => ({
    sessionId: 'bulk',
    cwd: f.cwd,
    path: join(f.cwd, `Gen${i}.java`),
    version: 'gone',
  }))
  const retriesPath = join(f.dataDir, 'pisper-workspace-asset-retries.json')
  await writeFile(
    retriesPath,
    JSON.stringify({ files: [...bulk, { ...keptFile, sessionId: 'normal', cwd: f.cwd }] }),
  )
  const tracker = f.createTracker()
  assert.deepEqual(await tracker.drain('bulk'), [])
  assert.equal(f.attempts.length, 0)
  assert.deepEqual(
    (await tracker.drain('normal')).map(({ path }) => path),
    [kept],
  )
  await tracker.saving
  assert.deepEqual(JSON.parse(await readFile(retriesPath, 'utf8')).files, [])
})

test('archiving commits the asset index once per command without holding back its result', async (t) => {
  const blocked = deferred()
  let commits = 0
  const f = await fixture(t, undefined, {
    commit: () => {
      commits += 1
      return blocked.promise
    },
  })
  const result = await f.run('session-a', async () => {
    for (const name of ['one.md', 'two.md', 'three.md']) await writeFile(join(f.cwd, name), name)
    return 'done'
  })
  // 索引还在写，命令结果已经回来；三个文件只提交一次。
  assert.equal(result, 'done')
  assert.equal(f.archived.length, 3)
  await nextTick()
  assert.equal(commits, 1)
  let drained = false
  const draining = f.tracker.drain('session-a').then((assets) => {
    drained = true
    return assets
  })
  await nextTick()
  assert.equal(drained, false, '本轮收尾要等索引写完')
  blocked.resolve()
  assert.equal((await draining).length, 3)
  assert.equal(commits, 1)
})
