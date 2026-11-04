// 执行节点（B6）：配对与令牌、连接中枢、节点上的文件与命令操作（目录限制、git 白名单）、
// 经节点创建 worktree 并跑完一个 Bot 任务、节点接口的鉴权，以及远程会话的工具。
// 节点与大脑在同一进程里经内存“轮询”相连，不开端口、不联网。
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  ExecutorNode,
  executorCommand,
  globToRegExp,
  normalizeFingerprint,
} from '../../packages/pisper/lib/executor-node.mjs'
import { createApiHandler } from '../http/api-handler.mjs'
import { authorizeRemoteRequest } from '../remote-auth.mjs'
import {
  createExecutorTools,
  normalizeRemoteExecutor,
  REMOTE_EXECUTOR_TOOL_NAMES,
} from '../runtime/executor-tools.mjs'
import { runToolBlocker } from '../runtime/tool-activation.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import { BotService } from '../services/bots/bot-service.mjs'
import { BotTaskService } from '../services/bots/bot-task-service.mjs'
import { EnvironmentService, pathWithinRoots } from '../services/bots/environment-service.mjs'
import { NodeExecutor } from '../services/bots/executor.mjs'
import { ExecutorHub } from '../services/bots/executor-hub.mjs'
import { WorktreeManager } from '../services/bots/worktree-manager.mjs'

const IDENTITY = ['-c', 'user.name=Test', '-c', 'user.email=test@example.com']

function git(cwd, ...args) {
  return execFileSync('git', [...IDENTITY, ...args], { cwd, encoding: 'utf8' }).trim()
}

async function until(predicate, timeoutMs = 5_000) {
  const started = Date.now()
  while (!(await predicate())) {
    if (Date.now() - started > timeoutMs) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

// 一台“远端”电脑：允许目录里有一个 git 仓库，另有一个允许目录之外的秘密文件。
async function machine(t, { shell = true } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-executor-'))
  const allowed = join(directory, 'allowed')
  const repo = join(allowed, 'repo')
  await mkdir(repo, { recursive: true })
  git(repo, 'init', '-q', '-b', 'main')
  git(repo, 'config', 'user.name', 'Test')
  git(repo, 'config', 'user.email', 'test@example.com')
  await writeFile(join(repo, 'README.md'), 'hello\n')
  git(repo, 'add', '-A')
  git(repo, 'commit', '-q', '-m', 'init')
  await writeFile(join(directory, 'secret.txt'), 'top secret\n')
  const node = new ExecutorNode({
    roots: [allowed],
    worktreeRoot: join(directory, 'node-home', 'worktrees'),
    shell,
  })
  const hub = new ExecutorHub()
  const environment = {
    id: 'env_testmachine1',
    name: '测试机',
    capabilities: { shell, git: true },
    ...node.report(),
  }
  const disconnect = connect(hub, environment.id, node)
  t.after(async () => {
    await disconnect()
    hub.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  return { directory, allowed, repo, node, hub, environment, disconnect }
}

// 节点的轮询循环，换成直接调用中枢：领请求、执行、交回结果与输出。
function connect(hub, environmentId, node) {
  let stopped = false
  const loop = (async () => {
    while (!stopped) {
      const requests = await hub.poll(environmentId, { waitMs: 30 })
      for (const request of requests)
        void node
          .handle(request, (data) =>
            hub.deliver(environmentId, [
              { id: request.id, chunk: Buffer.from(data).toString('base64') },
            ]),
          )
          .then((result) => {
            if (request.op !== 'cancel') hub.deliver(environmentId, [{ id: request.id, ...result }])
          })
    }
  })()
  return async () => {
    stopped = true
    await loop
  }
}

test('pairing codes are one-time and short-lived; tokens are stored only as digests', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-environments-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let now = Date.parse('2026-11-03T00:00:00.000Z')
  const path = join(directory, 'pisper-environments.json')
  const service = new EnvironmentService({ path, now: () => now })
  await service.init()
  const { code } = service.createPairing()
  assert.match(code, /^[A-Z2-9]{4}-[A-Z2-9]{4}$/)
  await assert.rejects(
    service.pair({ code, roots: [] }),
    (error) => error.code === 'environment_roots_required',
  )
  // 配对码用过一次就作废（上面那次虽然失败也已消耗）。
  await assert.rejects(service.pair({ code, roots: ['/work'] }), { code: 'pairing_code_invalid' })

  const fresh = service.createPairing()
  const paired = await service.pair({
    code: fresh.code.toLowerCase(),
    name: '构建机\u0007',
    roots: ['/work', 'relative/path', '/work'],
    worktreeRoot: '/home/me/.pisper-executor/worktrees',
    capabilities: { shell: true, git: true },
  })
  assert.equal(paired.environment.name, '构建机')
  assert.deepEqual(paired.environment.roots, ['/work'])
  assert.equal('tokenHash' in paired.environment, false)
  assert.equal(service.authenticate(paired.token)?.id, paired.environment.id)
  assert.equal(service.authenticate(`${paired.environment.id}.wrong`), null)
  assert.equal(service.authenticate('nonsense'), null)

  const stored = await readFile(path, 'utf8')
  assert.equal(stored.includes(paired.token.split('.')[1]), false)
  if (process.platform !== 'win32') assert.equal((await stat(path)).mode & 0o777, 0o600)

  // 过期的配对码不能用。
  const late = service.createPairing()
  now += 11 * 60_000
  await assert.rejects(service.pair({ code: late.code, roots: ['/w'] }), {
    code: 'pairing_code_invalid',
  })

  await assert.rejects(service.remove(paired.environment.id, { inUseBy: ['审查员'] }), {
    code: 'environment_in_use',
  })
  assert.equal(await service.remove(paired.environment.id), true)
  assert.equal(service.authenticate(paired.token), null)

  assert.equal(pathWithinRoots('/work/repo', ['/work']), true)
  assert.equal(pathWithinRoots('/workshop', ['/work']), false)
  assert.equal(pathWithinRoots('repo', ['/work']), false)
})

test('the node only touches files inside its allowed folders, by real path', async (t) => {
  const { allowed, repo, directory, hub, environment } = await machine(t)
  const executor = new NodeExecutor({ hub, environment })
  await until(() => executor.online())
  const ops = executor.operations()

  await ops.write.writeFile(join(repo, 'src', 'a.txt'), 'A\n')
  assert.equal((await ops.read.readFile(join(repo, 'src', 'a.txt'))).toString(), 'A\n')
  assert.deepEqual((await ops.ls.readdir(repo)).sort(), ['.git', 'README.md', 'src'])
  assert.equal((await ops.ls.stat(join(repo, 'src'))).isDirectory(), true)
  assert.equal(await ops.find.exists(join(repo, 'nope')), false)
  assert.deepEqual(
    (await ops.find.glob('**/*.txt', repo, { ignore: ['**/node_modules/**'], limit: 10 })).sort(),
    ['src/a.txt'],
  )

  await assert.rejects(ops.read.readFile(join(directory, 'secret.txt')), {
    code: 'path_not_allowed',
  })
  await assert.rejects(ops.write.writeFile(join(allowed, '..', 'evil.txt'), 'x'), {
    code: 'path_not_allowed',
  })
  // 允许目录里的符号链接指向外面：按真实路径检查，读不到。
  await symlink(directory, join(allowed, 'escape'))
  await assert.rejects(ops.read.readFile(join(allowed, 'escape', 'secret.txt')), {
    code: 'path_not_allowed',
  })
  await assert.rejects(ops.read.readFile('relative.txt'), { code: 'path_invalid' })
})

test('git on the node only allows the worktree subcommands, without global overrides', async (t) => {
  const { repo, directory, hub, environment } = await machine(t)
  const executor = new NodeExecutor({ hub, environment })
  await until(() => executor.online())
  assert.equal((await executor.exec('git', ['rev-parse', '--show-toplevel'], repo)).code, 0)
  for (const args of [
    ['-C', directory, 'status'],
    ['-c', 'core.sshCommand=touch /tmp/pwned', 'status'],
    ['config', 'core.hooksPath', '/tmp'],
    ['log'],
    ['worktree', 'add', '-b', 'x', '../../outside'],
    ['worktree', 'add', '-b', 'x', join(directory, 'outside')],
  ])
    await assert.rejects(executor.exec('git', args, repo), (error) =>
      ['command_not_allowed', 'path_not_allowed'].includes(error.code),
    )
  await assert.rejects(executor.exec('git', ['status'], directory), { code: 'path_not_allowed' })
  await assert.rejects(executor.exec('rm', ['-rf', '/'], repo), { code: 'command_not_allowed' })
})

test('shell commands stream output, report exit codes and stop when aborted', async (t) => {
  const { repo, hub, environment } = await machine(t)
  const executor = new NodeExecutor({ hub, environment })
  await until(() => executor.online())
  const chunks = []
  const result = await executor
    .operations()
    .bash.exec('echo hello; echo oops 1>&2; exit 3', repo, { onData: (data) => chunks.push(data) })
  assert.equal(result.exitCode, 3)
  assert.match(Buffer.concat(chunks).toString(), /hello/)
  assert.match(Buffer.concat(chunks).toString(), /oops/)

  const controller = new AbortController()
  const started = Date.now()
  const running = executor
    .operations()
    .bash.exec('sleep 30', repo, { onData: () => {}, signal: controller.signal })
  setTimeout(() => controller.abort(), 100)
  await assert.rejects(running, { code: 'aborted' })
  assert.ok(Date.now() - started < 5_000)
})

test('a node without --no-shell refuses commands; offline nodes fail fast', async (t) => {
  const { repo, hub, environment, disconnect } = await machine(t, { shell: false })
  const executor = new NodeExecutor({ hub, environment })
  await until(() => executor.online())
  await assert.rejects(executor.operations().bash.exec('echo hi', repo, {}), {
    code: 'shell_disabled',
  })
  await disconnect()
  const offline = new NodeExecutor({ hub: new ExecutorHub(), environment })
  assert.equal(offline.online(), false)
  await assert.rejects(offline.exec('git', ['status'], repo), { code: 'executor_offline' })
})

test('a bot task on a node waits for it, then runs in the bot folder on that machine', async (t) => {
  const { repo, hub, environment, directory } = await machine(t)
  const executor = new NodeExecutor({ hub, environment })
  const nodeWorktrees = new WorktreeManager({ executor, root: environment.worktreeRoot })
  let online = false
  const bot = {
    id: 'remote',
    ...normalizeBotInput({
      name: '远程',
      workspace: { environmentId: environment.id, cwd: repo },
    }),
  }
  const calls = []
  const service = new BotTaskService({
    path: join(directory, 'tasks.json'),
    getBot: () => bot,
    worktreesFor: (id) => {
      assert.equal(id, environment.id)
      return nodeWorktrees
    },
    isEnvironmentOnline: () => online,
    runAgent: async ({ cwd, environmentId }) => {
      calls.push({ cwd, environmentId })
      // 代理的工具经节点写文件。
      await executor.operations().write.writeFile(join(cwd, 'fix.txt'), 'fixed\n')
      return { text: '改好了', sessionId: 'thread' }
    },
    stopAgent: async () => {},
  })
  t.after(() => service.dispose())
  await service.init()
  const task = await service.dispatch('remote', { text: '修一下' })
  assert.equal(service.get(task.id).environmentId, environment.id)
  // 电脑不在线：留在排队里，侧栏写“等它的电脑”。
  await new Promise((resolve) => setTimeout(resolve, 50))
  assert.equal(service.get(task.id).status, 'queued')
  assert.equal(service.activity('remote').waiting, 'environment')

  online = true
  service.schedule()
  // Bot 不依赖 git：在那台电脑上直接在 Bot 的目录里跑，不开分支、不进审阅。
  await until(() => service.get(task.id).status === 'done', 15_000)
  const stored = service.get(task.id)
  assert.equal(calls[0].environmentId, environment.id)
  assert.equal(calls[0].cwd, repo)
  assert.equal(stored.workspace.kind, 'shared')
  assert.equal(stored.result.summary, '改好了')
  assert.equal(await readFile(join(repo, 'fix.txt'), 'utf8'), 'fixed\n')
})

test('a connection lost mid-run marks the task interrupted, not failed', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-executor-lost-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const bot = {
    id: 'remote',
    ...normalizeBotInput({
      name: '远程',
      delivery: 'reply-only',
      workspace: { environmentId: 'env_testmachine1', cwd: '/work', isolation: 'shared' },
    }),
  }
  const service = new BotTaskService({
    path: join(directory, 'tasks.json'),
    getBot: () => bot,
    worktreesFor: () => null,
    isEnvironmentOnline: () => true,
    runAgent: async () => {
      throw Object.assign(new Error('这台电脑太久没有回应'), { code: 'executor_timeout' })
    },
    stopAgent: async () => {},
  })
  t.after(() => service.dispose())
  await service.init()
  const task = await service.dispatch('remote', { text: '跑一下测试' })
  await until(() => service.get(task.id).status !== 'queued' && !service.running.size)
  assert.equal(service.get(task.id).status, 'interrupted')
})

test('bots can only point at a paired node and a folder inside its roots', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-env-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const environments = new Map([
    ['env_testmachine1', { id: 'env_testmachine1', roots: ['/work'], name: '构建机' }],
  ])
  const service = new BotService({
    path: join(directory, 'bots.json'),
    validateDirectory: async (input) => input,
    getEnvironment: (id) => environments.get(id) || null,
  })
  await service.init()
  const created = await service.create({
    name: '远程',
    workspace: { environmentId: 'env_testmachine1', cwd: '/work/repo' },
  })
  assert.equal(created.workspace.environmentId, 'env_testmachine1')
  await assert.rejects(
    service.create({ name: '越界', workspace: { environmentId: 'env_testmachine1', cwd: '/etc' } }),
    { code: 'directory_not_allowed' },
  )
  await assert.rejects(
    service.create({ name: '没有', workspace: { environmentId: 'env_unknown123', cwd: '/work' } }),
    { code: 'environment_not_found' },
  )
  await assert.rejects(
    service.create({ name: '乱写', workspace: { environmentId: '../../x', cwd: '/work' } }),
    { code: 'environment_not_found' },
  )
})

test('node endpoints skip device tokens but require the node token and the Labs switch', async () => {
  const response = () => ({
    status: 0,
    body: '',
    headers: {},
    destroyed: false,
    writableEnded: false,
    headersSent: false,
    writeHead(status) {
      this.status = status
    },
    flushHeaders() {},
    write(chunk = '') {
      this.body += chunk
    },
    end(chunk = '') {
      this.body += chunk
      this.writableEnded = true
    },
  })
  const remoteAccess = { authenticateToken: () => null, trackResponse: () => {} }
  for (const path of ['/api/executor/pair', '/api/executor/poll', '/api/executor/results'])
    assert.equal(
      authorizeRemoteRequest(
        { method: 'POST', headers: {} },
        response(),
        new URL(`https://x${path}`),
        { remoteAccess },
      ),
      false,
      path,
    )
  // 界面用的电脑管理接口仍然要设备令牌。
  const blocked = response()
  assert.equal(
    authorizeRemoteRequest(
      { method: 'GET', headers: {} },
      blocked,
      new URL('https://x/api/environments'),
      { remoteAccess },
    ),
    true,
  )

  let enabled = true
  const delivered = []
  const runtime = {
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => enabled,
    authenticateExecutor: (token) =>
      token === 'env_testmachine1.good' ? { id: 'env_testmachine1' } : null,
    deliverExecutorResults: (environment, results) => {
      delivered.push({ environment, results })
      return 1
    },
  }
  const handler = createApiHandler(runtime)
  const post = async (path, token, body) => {
    const output = response()
    const payload = Buffer.from(JSON.stringify(body))
    await handler(
      {
        method: 'POST',
        headers: token ? { authorization: `Bearer ${token}` } : {},
        once() {},
        off() {},
        async *[Symbol.asyncIterator]() {
          yield payload
        },
      },
      output,
      new URL(`http://localhost${path}`),
    )
    return { status: output.status, body: JSON.parse(output.body) }
  }
  assert.equal((await post('/api/executor/results', 'env_testmachine1.bad', {})).status, 401)
  assert.equal((await post('/api/executor/results', '', {})).status, 401)
  const ok = await post('/api/executor/results', 'env_testmachine1.good', {
    results: [{ id: 'r' }],
  })
  assert.equal(ok.status, 200)
  assert.equal(delivered[0].environment.id, 'env_testmachine1')
  enabled = false
  assert.equal((await post('/api/executor/results', 'env_testmachine1.good', {})).status, 409)
})

test('remote sessions get node-backed file and shell tools and nothing local-only', async () => {
  const calls = []
  const executor = {
    capabilities: { shell: true },
    operations: () => ({
      read: {
        readFile: async (path) => {
          calls.push(['read', path])
          return Buffer.from('remote content\n')
        },
        access: async () => {},
      },
      edit: {
        readFile: async () => Buffer.from(''),
        writeFile: async () => {},
        access: async () => {},
      },
      write: { writeFile: async () => {}, mkdir: async () => {} },
      ls: {
        exists: async () => true,
        stat: async () => ({ isDirectory: () => true }),
        readdir: async () => [],
      },
      find: { exists: async () => true, glob: async () => [] },
      bash: { exec: async () => ({ exitCode: 0 }) },
    }),
  }
  const tools = await createExecutorTools('/remote/repo', executor, [
    'read',
    'edit',
    'bash',
    'grep',
  ])
  assert.deepEqual(
    tools.map((tool) => tool.name),
    ['read', 'edit', 'bash'],
  )
  const read = tools.find((tool) => tool.name === 'read')
  const result = await read.execute('call-1', { path: 'src/a.txt' })
  assert.deepEqual(calls[0], ['read', '/remote/repo/src/a.txt'])
  assert.match(JSON.stringify(result.content), /remote content/)

  const blocked = runToolBlocker({ executorToolNames: [...REMOTE_EXECUTOR_TOOL_NAMES] })
  for (const name of ['grep', 'browser_automation', 'memory_search', 'spawn_agent', 'mcp_manage'])
    assert.equal(blocked(name), true, name)
  for (const name of ['read', 'bash', 'call_tool', 'update_plan'])
    assert.equal(blocked(name), false)

  assert.deepEqual(normalizeRemoteExecutor({ environmentId: 'env_testmachine1', cwd: '/w' }), {
    environmentId: 'env_testmachine1',
    cwd: '/w',
  })
  assert.equal(normalizeRemoteExecutor({ environmentId: 'local', cwd: '/w' }), null)
  assert.equal(normalizeRemoteExecutor({ environmentId: 'env_testmachine1', cwd: 'w' }), null)
})

test('the executor CLI pins the brain certificate and explains itself', async () => {
  const lines = []
  assert.equal(await executorCommand(['--help'], { log: (line) => lines.push(line) }), 0)
  assert.match(lines.join('\n'), /pisper executor pair/)
  await assert.rejects(executorCommand(['pair', '--brain', 'https://x']), /--fingerprint/)
  await assert.rejects(
    executorCommand([], { env: { PISPER_EXECUTOR_HOME: join(tmpdir(), 'pisper-no-such-home') } }),
    /还没有配对/,
  )
  assert.equal(
    normalizeFingerprint('sha256:' + 'ab'.repeat(32)),
    `SHA256:${Array(32).fill('AB').join(':')}`,
  )
  assert.equal(normalizeFingerprint('SHA256:short'), '')
  assert.ok(globToRegExp('**/*.ts').test('src/a/b.ts'))
  assert.ok(globToRegExp('**/*.ts').test('b.ts'))
  assert.ok(!globToRegExp('*.ts').test('src/b.ts'))
  assert.ok(globToRegExp('src/{a,b}.js').test('src/b.js'))
})
