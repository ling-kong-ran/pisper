// SSH 执行器：纯函数直接测；其余连本机起的真实 sshd（只用临时密钥，不碰 ~/.ssh），没有 sshd 的环境跳过。
import assert from 'node:assert/strict'
import { execFile, spawn, spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir, userInfo } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { promisify } from 'node:util'
import { ExecutorError } from '../services/bots/executor.mjs'
import {
  classifySshFailure,
  shellQuote,
  SshExecutor,
  sshArguments,
  toRemotePath,
} from '../services/ssh-executor.mjs'
import {
  isSshMachineId,
  normalizeSshConnection,
  SshMachineService,
} from '../services/ssh-machine-service.mjs'

const run = promisify(execFile)
const SSHD = ['/usr/sbin/sshd', '/usr/local/sbin/sshd'].find((path) => existsSync(path))
const hasSsh =
  spawnSync('ssh', ['-V']).status === 0 && spawnSync('ssh-keygen', ['-?']).error === undefined
const skipReal = process.platform === 'win32' || !SSHD || !hasSsh ? '这个环境没有 sshd' : false

test('shellQuote keeps every byte literal', async () => {
  for (const value of ["it's", '$(touch /tmp/x)', '`id`', 'a b', 'a\nb', '-rf', '"q"', '*']) {
    if (process.platform === 'win32') break
    const { stdout } = await run('sh', ['-c', `printf %s ${shellQuote(value)}`])
    assert.equal(stdout, value)
  }
  assert.throws(() => shellQuote('a\0b'), ExecutorError)
})

test('a Windows brain path goes back to the POSIX path it came from', () => {
  assert.equal(toRemotePath('C:\\srv\\app\\a.txt', 'win32'), '/srv/app/a.txt')
  assert.equal(toRemotePath('/srv/app/a.txt', 'win32'), '/srv/app/a.txt')
  assert.equal(toRemotePath('C:\\srv\\app', 'linux'), 'C:\\srv\\app')
})

test('connection input only takes hosts, never anything ssh could read as an option', () => {
  for (const target of [
    'prod',
    'root@1.2.3.4',
    'me@host.example.com',
    'deploy_user@web-1',
    '[::1]',
    'me@[fe80::1]',
  ])
    assert.equal(normalizeSshConnection({ target }).target, target)
  for (const target of [
    '-oProxyCommand=touch x',
    '',
    'a b',
    'host;id',
    '$(id)',
    'me@-oX',
    'a`b`',
    'a\nb',
    '@host',
  ])
    assert.throws(() => normalizeSshConnection({ target }), { code: 'invalid_target' }, target)
  assert.equal(normalizeSshConnection({ target: 'h', port: '2222' }).port, 2222)
  for (const port of [0, 70000, 'x', 1.5])
    assert.throws(() => normalizeSshConnection({ target: 'h', port }), { code: 'invalid_port' })
})

test('the arguments keep keys and ports from reaching the other machine', () => {
  const args = sshArguments({ target: 'me@host', port: 2200 }, { muxDirectory: '/m' })
  assert.deepEqual(args.slice(-2), ['--', 'me@host'])
  const joined = args.join(' ')
  for (const option of [
    'BatchMode=yes',
    'ForwardAgent=no',
    'ClearAllForwardings=yes',
    'StrictHostKeyChecking=accept-new',
  ])
    assert.ok(joined.includes(option), option)
  assert.ok(joined.includes('-p 2200'))
  assert.ok(joined.includes('ControlPath=/m/%C'))
  assert.ok(!sshArguments({ target: 'h' }).join(' ').includes('ControlPath'))
})

test('Windows and overlong paths do not use connection sharing', () => {
  const machine = { id: 'ssh_aaaaaaaa', name: 'x', target: 'h' }
  assert.equal(new SshExecutor({ machine, muxDirectory: '/m', platform: 'win32' }).muxDirectory, '')
  assert.equal(
    new SshExecutor({ machine, muxDirectory: `/${'x'.repeat(80)}`, platform: 'linux' })
      .muxDirectory,
    '',
  )
  assert.equal(
    new SshExecutor({ machine, muxDirectory: '/m', platform: 'linux' }).muxDirectory,
    '/m',
  )
})

test('ssh client failures become causes people can act on', () => {
  const cases = {
    'ssh: connect to host x port 22: Connection refused': 'connection_refused',
    'Permission denied (publickey).': 'auth_failed',
    'ssh: connect to host x port 22: Operation timed out': 'unreachable',
    'ssh: Could not resolve hostname nope: Name or service not known': 'unreachable',
    '@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@': 'host_key_changed',
    'Host key verification failed.': 'host_key_changed',
    'something else': 'connection_failed',
  }
  for (const [stderr, code] of Object.entries(cases)) assert.equal(classifySshFailure(stderr), code)
})

test('a missing ssh client is reported as such', async () => {
  const executor = new SshExecutor({
    machine: { id: 'ssh_aaaaaaaa', name: 'x', target: 'h' },
    spawnProcess: () => {
      const child = spawn(process.execPath, ['-e', ''], { stdio: 'pipe' })
      queueMicrotask(() => child.emit('error', Object.assign(new Error('x'), { code: 'ENOENT' })))
      return child
    },
  })
  await assert.rejects(executor.probe(), { code: 'ssh_client_missing' })
})

test('the machine list survives bad entries and stores no credentials', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-ssh-store-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const path = join(dir, 'machines.json')
  await writeFile(
    path,
    JSON.stringify({
      version: 1,
      machines: [
        null,
        { id: 'bad', target: 'h' },
        { id: 'ssh_aaaaaaaa', target: '-oX' },
        { id: 'ssh_bbbbbbbb', target: 'ok' },
      ],
    }),
  )
  const service = new SshMachineService({ path, dataDir: dir })
  await service.init()
  assert.deepEqual(
    service.list().map((machine) => machine.id),
    ['ssh_bbbbbbbb'],
  )
  const added = await service.add({
    target: 'me@host',
    port: 22,
    name: ' 生产 ',
    password: 'nope',
    identityFile: '/x',
  })
  assert.ok(isSshMachineId(added.id))
  assert.equal(added.name, '生产')
  const onDisk = await readFile(path, 'utf8')
  assert.ok(!onDisk.includes('nope'))
  assert.ok(!onDisk.includes('identityFile'))
  assert.equal(await service.remove(added.id), true)
  assert.equal(await service.remove(added.id), false)
  await assert.rejects(service.executor(added.id).probe(), { code: 'machine_not_found' })
  await assert.rejects(service.executor(added.id).operations().read.readFile('/x'), {
    code: 'machine_not_found',
  })
})

async function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address()
      server.close(() => resolve(port))
    })
  })
}

async function startSshd(t) {
  // 放在短路径下：ControlPath 有长度上限。
  const dir = await mkdtemp(join(tmpdir(), 'pssh-'))
  await chmod(dir, 0o700)
  const port = await freePort()
  const keygen = (file) =>
    run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', join(dir, file)])
  await keygen('host_key')
  await keygen('client_key')
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
      'PasswordAuthentication no',
      'KbdInteractiveAuthentication no',
      'UsePAM no',
      'LogLevel ERROR',
      '',
    ].join('\n'),
  )
  const config = join(dir, 'ssh_config')
  await writeFile(
    config,
    [
      'Host *',
      `  IdentityFile ${join(dir, 'client_key')}`,
      '  IdentitiesOnly yes',
      `  UserKnownHostsFile ${join(dir, 'known_hosts')}`,
      '',
    ].join('\n'),
  )
  const daemon = spawn(SSHD, ['-D', '-e', '-f', join(dir, 'sshd_config')], { stdio: 'ignore' })
  t.after(async () => {
    daemon.kill()
    await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  })
  const machine = {
    id: 'ssh_testtest',
    name: 'test',
    target: `${userInfo().username}@127.0.0.1`,
    port,
  }
  const make = (overrides = {}) =>
    new SshExecutor({
      machine: { ...machine, ...overrides },
      muxDirectory: join(dir, 'm'),
      extraArguments: ['-F', config],
    })
  const executor = make()
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await executor.probe()
      break
    } catch (error) {
      if (attempt === 49) throw error
      await new Promise((resume) => setTimeout(resume, 100))
    }
  }
  const work = join(dir, 'work dir')
  await mkdir(work)
  return { dir, work, port, config, machine, executor, make, ops: executor.operations() }
}

const realTest = (name, body) => test(name, { skip: skipReal, timeout: 60_000 }, body)

realTest('probe reports the platform and home directory', async (t) => {
  const { executor } = await startSshd(t)
  const result = await executor.probe()
  assert.ok(result.platform.length > 0)
  assert.ok(result.home.startsWith('/'))
})

realTest('file operations round trip, including awkward names and binary content', async (t) => {
  const { ops, work } = await startSshd(t)
  const path = join(work, "it's a $(file).txt")
  await ops.write.writeFile(path, '你好\n第二行\n')
  assert.equal((await ops.read.readFile(path)).toString('utf8'), '你好\n第二行\n')
  await ops.read.access(path)
  await ops.edit.access(path)
  await ops.write.mkdir(join(work, 'a/b/c'))
  assert.equal(await ops.ls.exists(join(work, 'a/b/c')), true)
  assert.equal((await ops.ls.stat(join(work, 'a'))).isDirectory(), true)
  assert.equal((await ops.ls.stat(path)).isDirectory(), false)

  const bytes = Buffer.from([0, 1, 2, 254, 255, 10, 13, 0x1e])
  await writeFile(join(work, 'bin'), bytes)
  assert.deepEqual(await ops.read.readFile(join(work, 'bin')), bytes)

  assert.deepEqual((await ops.ls.readdir(work)).sort(), ['a', 'bin', "it's a $(file).txt"].sort())
  // 没有注入成功：引号里的 $(file) 没被执行。
  assert.equal(existsSync(join(work, 'file')), false)
})

realTest('file errors look like file errors', async (t) => {
  const { ops, work } = await startSshd(t)
  await assert.rejects(ops.read.readFile(join(work, 'nope')), { code: 'not_found' })
  await assert.rejects(ops.read.readFile(work), { code: 'is_directory' })
  await assert.rejects(ops.read.access(join(work, 'nope')), { code: 'not_found' })
  await assert.rejects(ops.ls.readdir(join(work, 'nope')), { code: 'not_found' })
  await assert.rejects(ops.ls.stat(join(work, 'nope')), { code: 'not_found' })
  assert.equal(await ops.ls.exists(join(work, 'nope')), false)
  const locked = join(work, 'locked')
  await writeFile(locked, 'x', { mode: 0o444 })
  if (process.getuid?.() !== 0)
    await assert.rejects(ops.edit.access(locked), { code: 'permission_denied' })
})

realTest('a listing saves a round trip per entry', async (t) => {
  const { executor, ops, work } = await startSshd(t)
  await mkdir(join(work, 'sub'))
  await writeFile(join(work, 'file.txt'), 'x')
  await ops.ls.readdir(work)
  assert.ok(executor.listCache.has(join(work, 'sub')))
  assert.equal((await ops.ls.stat(join(work, 'sub'))).isDirectory(), true)
  // 写入后缓存作废：不能拿旧的结果回答。
  await ops.write.writeFile(join(work, 'new.txt'), 'x')
  assert.equal(executor.listCache.size, 0)
})

realTest('glob follows the same rules as the execution node', async (t) => {
  const { ops, work } = await startSshd(t)
  await mkdir(join(work, 'src/deep'), { recursive: true })
  await mkdir(join(work, 'node_modules/pkg'), { recursive: true })
  await mkdir(join(work, '.git'))
  for (const name of [
    'src/a.ts',
    'src/deep/b.ts',
    'src/c.js',
    'node_modules/pkg/d.ts',
    '.git/e.ts',
    'top.ts',
  ])
    await writeFile(join(work, name), 'x')
  assert.deepEqual((await ops.find.glob('**/*.ts', work)).sort(), [
    'src/a.ts',
    'src/deep/b.ts',
    'top.ts',
  ])
  assert.deepEqual((await ops.find.glob('src/*.{ts,js}', work)).sort(), ['src/a.ts', 'src/c.js'])
  assert.deepEqual(await ops.find.glob('**/*.ts', work, { ignore: ['src/**'] }), ['top.ts'])
  assert.deepEqual(await ops.find.glob('node_modules/**/*.ts', work), ['node_modules/pkg/d.ts'])
  assert.equal((await ops.find.glob('**/*.ts', work, { limit: 1 })).length, 1)
})

realTest('commands stream output and report the exit code', async (t) => {
  const { ops, work } = await startSshd(t)
  const chunks = []
  const onData = (chunk) => chunks.push(chunk)
  const ok = await ops.bash.exec('pwd; echo out; echo err >&2; exit 7', work, { onData })
  assert.equal(ok.exitCode, 7)
  const text = Buffer.concat(chunks).toString('utf8')
  assert.match(text, /work dir\nout\nerr\n/)
  assert.ok(!text.includes('\u001e'), 'markers stay out of the output')
})

realTest('multi-line commands, quotes and big output arrive intact', async (t) => {
  const { ops, work } = await startSshd(t)
  const chunks = []
  await ops.bash.exec(`printf '%s\\n' "it's"\necho "two lines"\nseq 1 200000`, work, {
    onData: (chunk) => chunks.push(chunk),
  })
  const lines = Buffer.concat(chunks).toString('utf8').trimEnd().split('\n')
  assert.equal(lines[0], "it's")
  assert.equal(lines[1], 'two lines')
  assert.equal(lines.at(-1), '200000')
  assert.equal(lines.length, 200002)
})

realTest('a background process cannot hold the command open and keeps running', async (t) => {
  const { ops, work } = await startSshd(t)
  const startedAt = Date.now()
  // 安静的和不停输出的都一样：命令退出就算结束，后台进程不被结束。
  const pidFile = join(work, 'bg.pid')
  const result = await ops.bash.exec(
    `(while true; do echo tick; sleep 0.05; done) & echo $! > ${shellQuote(pidFile)}; echo started`,
    work,
    { onData: () => {} },
  )
  assert.equal(result.exitCode, 0)
  assert.ok(Date.now() - startedAt < 8_000, 'finished promptly')
  const pid = Number(await readFile(pidFile, 'utf8'))
  t.after(() => {
    try {
      process.kill(pid)
    } catch {}
  })
  // 输出管道断了之后循环可能因 SIGPIPE 结束，这是后台进程自己的事；这里只要工具已经返回。
  const quiet = await ops.bash.exec('sleep 30 & echo $!', work, { onData: () => {} })
  assert.equal(quiet.exitCode, 0)
})

realTest('a command that asks for input fails instead of hanging', async (t) => {
  const { ops, work } = await startSshd(t)
  const startedAt = Date.now()
  const result = await ops.bash.exec('cat; echo done', work, { onData: () => {} })
  assert.equal(result.exitCode, 0)
  assert.ok(Date.now() - startedAt < 8_000)
})

realTest('stopping ends the whole command on the other machine', async (t) => {
  const { ops, work } = await startSshd(t)
  const controller = new AbortController()
  const pidFile = join(work, 'stop.pid')
  const running = ops.bash.exec(`sh -c 'echo $$ > stop.pid; exec sleep 300' & wait`, work, {
    onData: () => {},
    signal: controller.signal,
  })
  for (let attempt = 0; attempt < 50 && !existsSync(pidFile); attempt += 1)
    await new Promise((resume) => setTimeout(resume, 100))
  const pid = Number(await readFile(pidFile, 'utf8'))
  assert.doesNotThrow(() => process.kill(pid, 0))
  controller.abort()
  await assert.rejects(running, /aborted/)
  for (let attempt = 0; attempt < 30; attempt += 1) {
    try {
      process.kill(pid, 0)
    } catch {
      return
    }
    await new Promise((resume) => setTimeout(resume, 100))
  }
  assert.fail('the remote process is still running')
})

realTest('a timeout ends the command and says so the way the bash tool expects', async (t) => {
  const { ops, work } = await startSshd(t)
  await assert.rejects(
    ops.bash.exec('sleep 30', work, { onData: () => {}, timeout: 1 }),
    /^Error: timeout:1$/,
  )
})

realTest('a missing working directory is not reported as a command result', async (t) => {
  const { ops, work } = await startSshd(t)
  await assert.rejects(
    ops.bash.exec('echo hi', join(work, 'nope'), { onData: () => {} }),
    /Working directory does not exist/,
  )
})

realTest('directories resolve on the other machine, including home', async (t) => {
  const { executor, work } = await startSshd(t)
  const { home } = await executor.probe()
  assert.equal(await executor.resolveDirectory('~'), home)
  assert.equal(await executor.resolveDirectory(''), home)
  assert.equal(await executor.resolveDirectory(work), work)
  await assert.rejects(executor.resolveDirectory(join(work, 'nope')), {
    code: 'machine_folder_missing',
  })
})

realTest('connection failures are classified from what ssh actually says', async (t) => {
  const { make, port, machine } = await startSshd(t)
  const closed = await freePort()
  await assert.rejects(make({ port: closed }).probe(), { code: 'connection_refused' })
  await assert.rejects(make({ target: `nobody-here-${process.pid}@127.0.0.1` }).probe(), {
    code: 'auth_failed',
  })
  assert.ok(port > 0 && machine.target)
})

realTest('a changed host key refuses the connection', async (t) => {
  const { dir, config, machine, port } = await startSshd(t)
  const knownHosts = join(dir, 'known_hosts')
  assert.ok((await stat(knownHosts)).size > 0, 'the first connection recorded the key')
  const original = await readFile(knownHosts, 'utf8')
  const other = join(dir, 'other_key')
  await run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', other])
  const otherKey = (await readFile(`${other}.pub`, 'utf8')).split(' ').slice(0, 2).join(' ')
  await writeFile(knownHosts, `[127.0.0.1]:${port} ${otherKey}\n`)
  const executor = new SshExecutor({ machine, extraArguments: ['-F', config] })
  await assert.rejects(executor.probe(), { code: 'host_key_changed' })
  assert.notEqual(original, await readFile(knownHosts, 'utf8'))
})

realTest('it also works without connection sharing, as on Windows', async (t) => {
  const { config, machine, work } = await startSshd(t)
  const executor = new SshExecutor({ machine, extraArguments: ['-F', config] })
  assert.equal(executor.muxDirectory, '')
  const ops = executor.operations()
  await ops.write.writeFile(join(work, 'plain.txt'), 'plain')
  assert.equal((await ops.read.readFile(join(work, 'plain.txt'))).toString(), 'plain')
  const chunks = []
  const { exitCode } = await ops.bash.exec('cat plain.txt', work, {
    onData: (chunk) => chunks.push(chunk),
  })
  assert.equal(exitCode, 0)
  assert.equal(Buffer.concat(chunks).toString(), 'plain')
})
