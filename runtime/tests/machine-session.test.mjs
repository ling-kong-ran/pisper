// 远程机器全链路：真实 sidecar + 回环上的真实 sshd + 假模型。工具确实跑在“对方”的目录里，
// 本机的占位目录什么也不留。系统的 ssh 只认真实家目录，所以用 PATH 里的包装脚本指定配置，不碰 ~/.ssh。
import assert from 'node:assert/strict'
import { execFile, spawn, spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { chmod, mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { createServer as createNetServer } from 'node:net'
import { tmpdir, userInfo } from 'node:os'
import { delimiter, join, resolve } from 'node:path'
import test from 'node:test'
import { promisify } from 'node:util'

const run = promisify(execFile)
const appRoot = resolve(process.env.PISPER_TEST_APP_ROOT || '.')
const SSHD = ['/usr/sbin/sshd', '/usr/local/sbin/sshd'].find((path) => existsSync(path))
const skip =
  process.platform === 'win32' || !SSHD || spawnSync('ssh', ['-V']).status !== 0
    ? '这个环境没有 sshd'
    : false

async function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createNetServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address()
      server.close(() => resolvePort(port))
    })
  })
}

async function startSshd(t, { passwords = false } = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'pms-'))
  await chmod(dir, 0o700)
  const port = await freePort()
  for (const name of ['host_key', 'client_key'])
    await run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', join(dir, name)])
  await writeFile(join(dir, 'authorized_keys'), await readFile(join(dir, 'client_key.pub')), {
    mode: 0o600,
  })
  await writeFile(
    join(dir, 'sshd_config'),
    [
      `Port ${port}`,
      'ListenAddress 127.0.0.1',
      `HostKey ${join(dir, 'host_key')}`,
      `AuthorizedKeysFile ${join(dir, 'authorized_keys')}`,
      `PidFile ${join(dir, 'sshd.pid')}`,
      'StrictModes no',
      // 密码登录：没有 root 就读不了系统的密码库，任何密码都会被拒——正好用来测“错了立刻失败”。
      `PasswordAuthentication ${passwords ? 'yes' : 'no'}`,
      'KbdInteractiveAuthentication no',
      'UsePAM no',
      'LogLevel ERROR',
      '',
    ].join('\n'),
  )
  await writeFile(
    join(dir, 'ssh_config'),
    [
      'Host *',
      `  IdentityFile ${join(dir, 'client_key')}`,
      '  IdentitiesOnly yes',
      `  UserKnownHostsFile ${join(dir, 'known_hosts')}`,
      '',
    ].join('\n'),
  )
  const bin = join(dir, 'bin')
  await mkdir(bin)
  const realSsh = spawnSync('sh', ['-c', 'command -v ssh']).stdout.toString().trim()
  await writeFile(
    join(bin, 'ssh'),
    `#!/bin/sh\nexec ${realSsh} -F ${join(dir, 'ssh_config')} "$@"\n`,
    {
      mode: 0o755,
    },
  )
  const daemon = spawn(SSHD, ['-D', '-e', '-f', join(dir, 'sshd_config')], { stdio: 'ignore' })
  t.after(async () => {
    daemon.kill()
    await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  })
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const probe = spawnSync(join(bin, 'ssh'), [
      '-o',
      'BatchMode=yes',
      '-o',
      'StrictHostKeyChecking=accept-new',
      '-p',
      String(port),
      `${userInfo().username}@127.0.0.1`,
      'true',
    ])
    if (probe.status === 0) break
    await new Promise((resume) => setTimeout(resume, 100))
  }
  const work = join(dir, 'remote project')
  await mkdir(work)
  return { dir, bin, port, work, target: `${userInfo().username}@127.0.0.1` }
}

// 假模型：按用户消息里的 TOOL:{...} 发工具调用，工具结果回来后说完成。记下收到的工具和系统提示词。
async function startModel(t) {
  const seen = { tools: null, system: '' }
  const server = createServer(async (req, res) => {
    if (req.url?.endsWith('/models')) {
      res.setHeader('content-type', 'application/json')
      res.end(JSON.stringify({ data: [{ id: 'fake-model', object: 'model' }] }))
      return
    }
    let body = ''
    for await (const chunk of req) body += chunk
    const parsed = JSON.parse(body || '{}')
    if (!parsed.stream) {
      res.setHeader('content-type', 'application/json')
      res.end(
        JSON.stringify({
          id: 'x',
          object: 'chat.completion',
          created: 1,
          model: 'fake-model',
          choices: [
            { index: 0, message: { role: 'assistant', content: '{}' }, finish_reason: 'stop' },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      )
      return
    }
    seen.tools = (parsed.tools || []).map((tool) => tool.function?.name)
    seen.system = JSON.stringify(
      parsed.messages?.find((message) => ['system', 'developer'].includes(message.role)) ?? '',
    )
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const chunk = (delta, finish = null) =>
      res.write(
        `data: ${JSON.stringify({ id: 'x', object: 'chat.completion.chunk', created: 1, model: 'fake-model', choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`,
      )
    const last = parsed.messages?.at(-1)
    if (last?.role === 'tool') {
      chunk({ role: 'assistant', content: '好了。' }, 'stop')
    } else {
      const text =
        typeof last?.content === 'string' ? last.content : JSON.stringify(last?.content ?? '')
      const match = text.match(/TOOL:(\w+):([A-Za-z0-9+/=]+)/)
      if (match) {
        const args = Buffer.from(match[2], 'base64').toString('utf8')
        chunk({
          role: 'assistant',
          tool_calls: [
            {
              index: 0,
              id: `call_${Date.now()}`,
              type: 'function',
              function: { name: match[1], arguments: args },
            },
          ],
        })
        chunk({}, 'tool_calls')
      } else chunk({ role: 'assistant', content: '收到。' }, 'stop')
    }
    res.write('data: [DONE]\n\n')
    res.end()
  })
  await new Promise((resume) => server.listen(0, '127.0.0.1', resume))
  t.after(() => server.close())
  return { port: server.address().port, seen }
}

async function startSidecar(t, { bin, modelPort }) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-machine-'))
  const home = await mkdtemp(join(tmpdir(), 'pisper-machine-home-'))
  const child = spawn(process.execPath, [join(appRoot, 'runtime', 'sidecar.mjs')], {
    cwd: appRoot,
    env: {
      ...process.env,
      HOME: home,
      PATH: `${bin}${delimiter}${process.env.PATH}`,
      PI_OFFLINE: '1',
      PISPER_AGENT_DIR: dataDir,
      PISPER_DESKTOP_TOKEN: 'machine-token',
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PISPER_WORKSPACE_DIR: home,
    },
    stdio: ['pipe', 'pipe', 'inherit'],
  })
  const ready = await new Promise((resolveReady, reject) => {
    let buffer = ''
    child.stdout.on('data', (chunk) => {
      buffer += chunk
      const match = buffer.match(/PISPER_SIDECAR_READY (.*)\n/)
      if (match) resolveReady(JSON.parse(match[1]))
    })
    child.once('exit', (code) => reject(new Error(`sidecar exited ${code}`)))
  })
  t.after(async () => {
    const exited = new Promise((resume) => child.once('exit', resume))
    child.stdin.write('shutdown\n')
    await Promise.race([exited, new Promise((resume) => setTimeout(resume, 8000))])
    if (child.exitCode === null) child.kill('SIGKILL')
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    await rm(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  })
  const bootstrap = await fetch(ready.bootstrapUrl, { redirect: 'manual' })
  const cookie = bootstrap.headers.get('set-cookie').split(';')[0]
  const origin = new URL(ready.url).origin
  const headers = { cookie, origin, 'content-type': 'application/json' }
  const call = async (method, path, body) => {
    const response = await fetch(`${origin}${path}`, {
      method,
      headers,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    })
    const text = await response.text()
    let data = null
    try {
      data = JSON.parse(text)
    } catch {}
    return { status: response.status, data }
  }
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if ((await call('GET', '/api/sessions')).status === 200) break
    await new Promise((resume) => setTimeout(resume, 100))
  }
  const provider = await call('POST', '/api/providers', {
    id: 'fake',
    name: 'Fake',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${modelPort}/v1`,
    model: 'fake-model',
    apiKey: 'not-a-real-key',
  })
  assert.equal(provider.status, 201)
  // 一轮对话：收到审批请求就按 approve 处理，返回工具结果和审批记录。
  const chat = async (sessionId, tool, args, { approve = true } = {}) => {
    const encoded = Buffer.from(JSON.stringify(args)).toString('base64')
    const response = await fetch(`${origin}/api/chat`, {
      method: 'POST',
      headers: { ...headers, accept: 'text/event-stream' },
      body: JSON.stringify({ sessionId, message: `TOOL:${tool}:${encoded}` }),
    })
    const result = { approvals: [], toolEnd: null, output: '', error: null }
    let buffer = ''
    const decoder = new TextDecoder()
    for await (const part of response.body) {
      buffer += decoder.decode(part, { stream: true })
      let index
      while ((index = buffer.indexOf('\n\n')) >= 0) {
        const frame = buffer.slice(0, index)
        buffer = buffer.slice(index + 2)
        const event = frame.match(/^event: (.*)$/m)?.[1]
        const line = frame.match(/^data: (.*)$/m)?.[1]
        if (!event || !line) continue
        const data = JSON.parse(line)
        if (event === 'permission_request') {
          result.approvals.push(data)
          await call('POST', `/api/sessions/${sessionId}/approvals/${data.id}`, {
            approved: approve,
          })
        } else if (event === 'tool_end') {
          result.toolEnd = data
          result.output = String(data.output ?? data.message ?? '')
        } else if (event === 'error') result.error = data.message
      }
    }
    return result
  }
  return { dataDir, call, chat }
}

test(
  'a session on an SSH machine runs its tools over there',
  { skip, timeout: 120_000 },
  async (t) => {
    const sshd = await startSshd(t)
    const model = await startModel(t)
    const { dataDir, call, chat } = await startSidecar(t, { bin: sshd.bin, modelPort: model.port })

    // 测试连接：通的、端口不通的、不合法的地址。
    const tested = await call('POST', '/api/machines/ssh/test', {
      target: sshd.target,
      port: sshd.port,
    })
    assert.equal(tested.data.ok, true, JSON.stringify(tested.data))
    assert.ok(tested.data.home.startsWith('/'))
    const closed = await call('POST', '/api/machines/ssh/test', {
      target: sshd.target,
      port: await freePort(),
    })
    assert.deepEqual([closed.data.ok, closed.data.code], [false, 'connection_refused'])
    const invalid = await call('POST', '/api/machines/ssh/test', {
      target: '-oProxyCommand=touch /tmp/x',
    })
    assert.deepEqual([invalid.status, invalid.data.code], [400, 'invalid_target'])

    // 添加后列出来；密码之类的字段不落盘。
    const added = await call('POST', '/api/machines/ssh', {
      name: '测试机',
      target: sshd.target,
      port: sshd.port,
      defaultCwd: sshd.work,
      password: 'must-not-be-stored',
    })
    assert.equal(added.status, 201)
    const machineId = added.data.id
    const stored = await readFile(join(dataDir, 'pisper-ssh-machines.json'), 'utf8')
    assert.ok(!stored.includes('must-not-be-stored'))
    const listed = await call('GET', '/api/machines')
    assert.equal(listed.data.machines[0].name, '测试机')
    assert.equal(listed.data.machines[0].kind, 'ssh')
    const checked = await call('POST', `/api/machines/${machineId}/check`)
    assert.equal(checked.data.ok, true)
    assert.equal((await call('GET', '/api/machines')).data.machines[0].online, true)

    // 建会话：目录不存在、机器不存在都明确报错，不留下空会话。
    const missing = await call('POST', '/api/sessions', {
      name: '错目录',
      machine: { id: machineId, cwd: join(sshd.work, 'nope') },
    })
    assert.deepEqual([missing.status, missing.data.code], [400, 'machine_folder_missing'])
    const unknown = await call('POST', '/api/sessions', {
      name: '错机器',
      machine: { id: 'ssh_doesnotexist', cwd: sshd.work },
    })
    assert.deepEqual([unknown.status, unknown.data.code], [400, 'machine_not_found'])
    assert.equal((await call('GET', '/api/sessions')).data.sessions?.length ?? 0, 0)

    const created = await call('POST', '/api/sessions', {
      name: '远程任务',
      machine: { id: machineId, cwd: sshd.work },
    })
    assert.equal(created.status, 201, JSON.stringify(created.data))
    const sessionId = created.data.id
    assert.deepEqual(created.data.machine, {
      id: machineId,
      kind: 'ssh',
      name: '测试机',
      cwd: sshd.work,
    })
    assert.equal(created.data.projectCwd, `machine://${machineId}${sshd.work}`)
    const placeholder = created.data.cwd
    assert.ok(placeholder.startsWith(dataDir), 'the local side only holds a placeholder folder')

    // 命令在对方目录跑，默认模式要审批。
    const ran = await chat(sessionId, 'bash', { command: 'pwd; echo from-remote' })
    assert.equal(ran.approvals.length, 1, 'approval mode still applies')
    assert.equal(ran.toolEnd.error, false, ran.output)
    assert.ok(ran.output.includes(sshd.work), ran.output)
    assert.ok(ran.output.includes('from-remote'), ran.output)

    // 模型知道自己在哪、那台机器是什么系统，手里只有能交给那台机器的工具。
    assert.match(model.seen.system, /remote machine/)
    assert.match(model.seen.system, /Operating system: Linux\. Commands run in its login shell/)
    assert.ok(model.seen.tools.includes('bash'))
    assert.ok(model.seen.tools.includes('read'))
    for (const tool of ['memory_search', 'grep', 'browser_automation'])
      assert.ok(!model.seen.tools.includes(tool), `${tool} must not be offered`)

    // 写文件：相对路径落在对方目录，本机占位目录是空的。
    const wrote = await chat(sessionId, 'write', { path: 'hello.txt', content: '你好' })
    assert.equal(wrote.toolEnd.error, false, wrote.output)
    assert.equal(await readFile(join(sshd.work, 'hello.txt'), 'utf8'), '你好')
    assert.deepEqual(await readdir(placeholder), [])
    const read = await chat(sessionId, 'read', { path: 'hello.txt' })
    assert.equal(read.toolEnd.error, false, read.output)
    const edited = await chat(sessionId, 'edit', {
      path: 'hello.txt',
      edits: [{ oldText: '你好', newText: '再见' }],
    })
    assert.equal(edited.toolEnd.error, false, edited.output)
    assert.equal(await readFile(join(sshd.work, 'hello.txt'), 'utf8'), '再见')
    const listing = await chat(sessionId, 'ls', {})
    assert.ok(listing.output.includes('hello.txt') || listing.toolEnd.error === false)

    // 拒绝审批：命令不执行。
    const denied = await chat(
      sessionId,
      'bash',
      { command: 'touch denied.txt' },
      { approve: false },
    )
    assert.equal(denied.approvals.length, 1)
    assert.equal(existsSync(join(sshd.work, 'denied.txt')), false)

    // 列表和看板都带着机器信息。
    const sessions = (await call('GET', '/api/sessions')).data.sessions
    assert.equal(sessions.find((item) => item.id === sessionId).machine.name, '测试机')
    const board = (await call('GET', '/api/sessions?view=board&limit=50&window=7d')).data
    const card = (board.items ?? board.sessions).find(
      (item) => (item.session?.id ?? item.id) === sessionId,
    )
    assert.equal((card.session ?? card).machine.cwd, sshd.work)

    // 侧聊跟着父会话：命令不能悄悄跑到本机。
    const side = await call('POST', `/api/sessions/${sessionId}/side-chat`)
    const sideId = side.data.session.id
    const sideRun = await chat(sideId, 'bash', { command: 'pwd' })
    assert.ok(sideRun.output.includes(sshd.work), `side chat ran in ${sideRun.output}`)

    // 机器移除后：会话还在，工具明确失败，不会退回本机执行。
    assert.equal((await call('DELETE', `/api/machines/${machineId}`)).status, 200)
    assert.equal((await call('DELETE', `/api/machines/${machineId}`)).status, 404)
    const after = await chat(sessionId, 'bash', { command: 'touch after-remove.txt' })
    assert.equal(after.toolEnd?.error, true, JSON.stringify(after))
    assert.match(after.output, /removed/)
    assert.equal(existsSync(join(placeholder, 'after-remove.txt')), false)
    assert.equal(existsSync(join(sshd.work, 'after-remove.txt')), false)
  },
)

test(
  'a workspace-write session only skips approval inside the remote folder',
  { skip, timeout: 90_000 },
  async (t) => {
    const sshd = await startSshd(t)
    const model = await startModel(t)
    const { call, chat } = await startSidecar(t, { bin: sshd.bin, modelPort: model.port })
    const added = await call('POST', '/api/machines/ssh', { target: sshd.target, port: sshd.port })
    const created = await call('POST', '/api/sessions', {
      name: '工作区写入',
      machine: { id: added.data.id, cwd: sshd.work },
    })
    const sessionId = created.data.id
    const mode = await call('PUT', `/api/sessions/${sessionId}/execution-mode`, {
      mode: 'workspace-write',
    })
    assert.ok(mode.status < 300, JSON.stringify(mode.data))
    const inside = await chat(sessionId, 'write', { path: 'inside.txt', content: 'x' })
    assert.equal(inside.approvals.length, 0, 'inside the remote folder needs no approval')
    assert.equal(existsSync(join(sshd.work, 'inside.txt')), true)
    const outside = join(sshd.dir, 'outside.txt')
    const blocked = await chat(
      sessionId,
      'write',
      { path: outside, content: 'x' },
      { approve: false },
    )
    assert.equal(blocked.approvals.length, 1, 'outside the remote folder asks first')
    assert.equal(existsSync(outside), false)
  },
)

// 递归找出数据目录里含有某段文字的文件（只看文本能读出的内容）。
async function filesContaining(directory, needle) {
  const found = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) found.push(...(await filesContaining(path, needle)))
    else if (entry.isFile() && (await readFile(path)).includes(needle)) found.push(entry.name)
  }
  return found
}

test(
  'a machine password goes in and never comes back out',
  { skip, timeout: 90_000 },
  async (t) => {
    const sshd = await startSshd(t, { passwords: true })
    const model = await startModel(t)
    const { dataDir, call } = await startSidecar(t, { bin: sshd.bin, modelPort: model.port })
    const password = 'pw-should-stay-private-123'
    const leaks = (value) => JSON.stringify(value).includes(password)

    const missing = await call('POST', '/api/machines/ssh', {
      target: sshd.target,
      auth: 'password',
    })
    assert.deepEqual([missing.status, missing.data.code], [400, 'invalid'])
    // 密码登录要先确认主机指纹：没带确认过的主机公钥就不保存，密码也不落盘。
    const unconfirmed = await call('POST', '/api/machines/ssh', {
      target: sshd.target,
      port: sshd.port,
      auth: 'password',
      password,
    })
    assert.deepEqual([unconfirmed.status, unconfirmed.data.code], [400, 'invalid'])
    assert.match(unconfirmed.data.error, /fingerprint/)
    assert.deepEqual(await filesContaining(dataDir, password), [])
    // 取指纹：请求里没有密码，拿回来的是公钥和算好的指纹。
    const fetched = await call('POST', '/api/machines/ssh/host-key', {
      target: sshd.target,
      port: sshd.port,
    })
    assert.equal(fetched.data.ok, true, JSON.stringify(fetched.data))
    assert.match(fetched.data.hostKey.fingerprint, /^SHA256:/)
    const hostKey = { type: fetched.data.hostKey.type, key: fetched.data.hostKey.key }
    const added = await call('POST', '/api/machines/ssh', {
      name: '密码机',
      target: sshd.target,
      port: sshd.port,
      auth: 'password',
      password,
      hostKey,
    })
    assert.equal(added.status, 201, JSON.stringify(added.data))
    assert.deepEqual([added.data.auth, added.data.passwordSet], ['password', true])
    assert.deepEqual(added.data.hostKey, {
      type: hostKey.type,
      fingerprint: fetched.data.hostKey.fingerprint,
    })
    assert.ok(!leaks(added.data))
    const id = added.data.id
    const listed = await call('GET', '/api/machines')
    assert.equal(listed.data.machines[0].passwordSet, true)
    assert.ok(!leaks(listed.data))
    // 只有密码文件里有它，且只有自己能读。
    assert.deepEqual(await filesContaining(dataDir, password), ['pisper-ssh-secrets.json'])
    assert.equal((await stat(join(dataDir, 'pisper-ssh-secrets.json'))).mode & 0o777, 0o600)

    // 测试已保存的机器：不重新输入，用存着的密码；对方拒绝时报认证失败，不回显密码。
    const tested = await call('POST', '/api/machines/ssh/test', {
      id,
      target: sshd.target,
      port: sshd.port,
      auth: 'password',
    })
    assert.deepEqual([tested.data.ok, tested.data.code], [false, 'auth_failed'])
    assert.ok(!leaks(tested.data))
    const checked = await call('POST', `/api/machines/${id}/check`)
    assert.deepEqual([checked.data.ok, checked.data.code], [false, 'auth_failed'])
    assert.ok(!leaks(checked.data))

    // 编辑：保留、换掉、清掉。
    const kept = await call('PATCH', `/api/machines/${id}`, { name: '改名', auth: 'password' })
    assert.deepEqual([kept.data.name, kept.data.passwordSet], ['改名', true])
    const replaced = await call('PATCH', `/api/machines/${id}`, { password: 'another-one-456' })
    assert.equal(replaced.data.passwordSet, true)
    assert.deepEqual(await filesContaining(dataDir, password), [])
    const cleared = await call('PATCH', `/api/machines/${id}`, { auth: 'key' })
    assert.deepEqual([cleared.data.auth, cleared.data.passwordSet], ['key', false])
    assert.deepEqual(await filesContaining(dataDir, 'another-one-456'), [])
    assert.equal((await call('PATCH', '/api/machines/ssh_doesnotexist', {})).status, 404)

    // 删除机器连同密码一起删。
    await call('PATCH', `/api/machines/${id}`, { auth: 'password', password, hostKey })
    assert.deepEqual(await filesContaining(dataDir, password), ['pisper-ssh-secrets.json'])
    assert.equal((await call('DELETE', `/api/machines/${id}`)).status, 200)
    assert.deepEqual(await filesContaining(dataDir, password), [])
  },
)
