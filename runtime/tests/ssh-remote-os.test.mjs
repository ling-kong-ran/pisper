// 远程机器跨系统与密码登录：
// - 每种对方系统（Linux / macOS 的 POSIX、Windows 的 cmd 或 PowerShell 登录）构造出的命令是纯函数，逐字比对；
// - 探测输出的解析、给模型的系统提示；
// - 密码登录的 ssh 选项、askpass 助手，以及密码不进命令行、不进错误、不进机器列表；
// - Windows 那边的 PowerShell 脚本在有 pwsh / powershell 的环境里真实跑一遍（模拟 ssh 把命令交给对方 shell），
//   没有时跳过。真正的 Windows 和 Mac 不在这里，见 docs/guide.html 的说明。
import assert from 'node:assert/strict'
import { execFile, spawn, spawnSync } from 'node:child_process'
import { EventEmitter } from 'node:events'
import { existsSync } from 'node:fs'
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir, userInfo } from 'node:os'
import { join } from 'node:path'
import { PassThrough } from 'node:stream'
import test from 'node:test'
import { promisify } from 'node:util'
import { machineSystemPrompt } from '../prompts/machines.mjs'
import { nodeMachineSystem } from '../runtime/executor-tools.mjs'
import {
  askpassEnvironment,
  normalizeSystem,
  parseProbeOutput,
  parseShellProbe,
  parseSshVersion,
  remoteRequest,
  shellQuote,
  SHELL_PROBE_COMMAND,
  SshExecutor,
  sshArguments,
  sshExecutable,
  systemFromProbe,
} from '../services/ssh-executor.mjs'
import {
  muxDirectoryFor,
  normalizeSshPassword,
  SshMachineService,
} from '../services/ssh-machine-service.mjs'
import {
  decodePowerShellPayload,
  POWERSHELL_COMMAND,
  POWERSHELL_SCRIPTS,
} from '../services/ssh-powershell.mjs'
import { ASKPASS_REFUSED, askpassAnswer, askpassPrompt } from '../workers/ssh-askpass.mjs'

const run = promisify(execFile)
const SECRET = 'S3cr3t pass"wörd $(id) `x`'

// ---------- 命令构造：纯函数，逐字比对 ----------

test('POSIX targets (Linux and macOS) get single-quoted sh scripts', () => {
  assert.deepEqual(remoteRequest('posix', 'readFile', { path: "/srv/it's a b" }), {
    command: `sh -c ${shellQuote(`[ -e '/srv/it'\\''s a b' ] || exit 44; [ -d '/srv/it'\\''s a b' ] && exit 45; exec cat -- '/srv/it'\\''s a b'`)}`,
    stdin: null,
  })
  assert.deepEqual(remoteRequest('posix', 'mkdir', { path: '/a' }), {
    command: `sh -c ${shellQuote(`mkdir -p -- '/a'`)}`,
    stdin: null,
  })
  const write = remoteRequest('posix', 'writeFile', { path: '/a.txt', data: '你好' })
  assert.equal(write.command, `sh -c ${shellQuote(`cat > '/a.txt'`)}`)
  assert.deepEqual(write.stdin, Buffer.from('你好'))
  assert.equal(
    remoteRequest('posix', 'resolveDirectory', { path: '~/my dir' }).command,
    `sh -c ${shellQuote(`cd "$HOME"/'my dir' 2>/dev/null || exit 44; pwd`)}`,
  )
  // 停止：/proc 没有（macOS）时用 ps 取进程组，kill -s TERM -- -pg 在 dash、bash 3.2、zsh 都认。
  assert.equal(
    remoteRequest('posix', 'kill', { pid: 123 }).command,
    `sh -c ${shellQuote(
      `pg=$(cut -d' ' -f5 /proc/123/stat 2>/dev/null); [ -n "$pg" ] || pg=$(ps -o pgid= -p 123 2>/dev/null | tr -d ' '); if [ -n "$pg" ] && [ "$pg" != 0 ] && [ "$pg" != 1 ]; then kill -s TERM -- "-$pg" 2>/dev/null; else kill -s TERM -- 123 2>/dev/null; fi`,
    )}`,
  )
  const exec = remoteRequest('posix', 'exec', { cwd: '/w', nonce: 'abc', command: 'echo "hi"' })
  assert.equal(
    exec.command,
    `sh -c ${shellQuote(
      `printf '\\036abcP%s\\036' "$$"; cd '/w' 2>/dev/null || exit 97; "\${SHELL:-/bin/sh}" -lc 'echo "hi"' </dev/null 2>&1; rc=$?; printf '\\036abcE%s\\036' "$rc"`,
    )}`,
  )
})

test('Windows targets always get the same encoded PowerShell command; everything else goes on stdin', () => {
  const encoded = POWERSHELL_COMMAND.split(' ').at(-1)
  assert.equal(
    POWERSHELL_COMMAND,
    `powershell -NoProfile -NonInteractive -EncodedCommand ${encoded}`,
  )
  // cmd 和 PowerShell 都不会解释 base64 里的字符：没有引号、空格、% $ & | < >。
  assert.match(encoded, /^[A-Za-z0-9+/=]+$/)
  assert.match(Buffer.from(encoded, 'base64').toString('utf16le'), /OpenStandardInput/)
  const path = "C:\\Users\\me\\it's ’odd’ & $(x) %PATH%.txt"
  const read = remoteRequest('windows', 'readFile', { path })
  assert.equal(read.command, POWERSHELL_COMMAND)
  const decoded = decodePowerShellPayload(read.stdin)
  assert.equal(decoded.script, POWERSHELL_SCRIPTS.readFile)
  assert.deepEqual(decoded.args, { path })
  assert.equal(decoded.data.length, 0)
  // 路径只作为 JSON 数据进去，脚本本身不变。
  assert.ok(!decoded.script.includes(path))
  const bytes = Buffer.from([0, 13, 10, 255, 0x1e])
  const write = decodePowerShellPayload(
    remoteRequest('windows', 'writeFile', { path: 'C:\\a.bin', data: bytes }).stdin,
  )
  assert.deepEqual(
    [write.script, write.args, write.data],
    [POWERSHELL_SCRIPTS.writeFile, { path: 'C:\\a.bin' }, bytes],
  )
  // 命令文本也走 stdin：不受 cmd 8191 字符的限制，不经任何 shell 的引号规则。
  const command = `Write-Output "a & b | c"\n${'x'.repeat(20_000)}`
  const exec = decodePowerShellPayload(
    remoteRequest('windows', 'exec', { cwd: 'C:\\w', nonce: 'n1', data: command }).stdin,
  )
  assert.deepEqual(exec.args, { cwd: 'C:\\w', nonce: 'n1' })
  assert.equal(exec.data.toString('utf8'), command)
  assert.match(exec.script, /taskkill|EncodedCommand/)
  assert.deepEqual(
    decodePowerShellPayload(remoteRequest('windows', 'kill', { pid: 4321 }).stdin).args,
    { pid: 4321 },
  )
  assert.match(POWERSHELL_SCRIPTS.kill, /taskkill\.exe'\) \/PID \(\[int\]\$a\.pid\) \/T \/F/)
})

test('the login shell is told apart from how one echo line expands', () => {
  assert.equal(SHELL_PROBE_COMMAND, 'echo "PISPER_SSH_SHELL_%OS%_$PSHOME"')
  // cmd：展开 %OS%，引号原样打印。
  assert.deepEqual(parseShellProbe('"PISPER_SSH_SHELL_Windows_NT_$PSHOME"\r\n'), {
    family: 'windows',
    loginShell: 'cmd',
  })
  // Windows 上的 PowerShell：展开 $PSHOME。
  assert.deepEqual(
    parseShellProbe('PISPER_SSH_SHELL_%OS%_C:\\Windows\\System32\\WindowsPowerShell\\v1.0\r\n'),
    { family: 'windows', loginShell: 'powershell' },
  )
  // bash、zsh、dash、fish：两种都不展开，$PSHOME 是空的。
  assert.deepEqual(parseShellProbe('Welcome!\nPISPER_SSH_SHELL_%OS%_\n'), {
    family: 'posix',
    loginShell: '',
  })
  // Linux / macOS 上把 PowerShell 设成了登录 shell：说不支持。
  assert.equal(
    parseShellProbe('PISPER_SSH_SHELL_%OS%_/opt/microsoft/powershell/7').family,
    'unsupported',
  )
  assert.equal(parseShellProbe('PSHOME: Undefined variable.'), null)
})

test('the echo line really expands that way in the POSIX shells here', async () => {
  for (const shell of ['sh', 'bash', 'dash', 'zsh', 'fish']) {
    if (spawnSync('sh', ['-c', `command -v ${shell}`]).status !== 0) continue
    const { stdout } = await run(shell, ['-c', SHELL_PROBE_COMMAND], {
      env: { ...process.env, PSHOME: '' },
    })
    assert.deepEqual(parseShellProbe(stdout), { family: 'posix', loginShell: '' }, shell)
  }
})

test('probe output becomes a system record; stored records are not trusted blindly', () => {
  const linux = parseProbeOutput(
    'motd\nPISPER_SSH_PROBE\nos=Linux\nhome=/home/me\nshell=/bin/bash\n',
  )
  assert.deepEqual(linux, {
    platform: 'Linux',
    version: '',
    home: '/home/me',
    shell: '/bin/bash',
    missing: [],
  })
  assert.deepEqual(systemFromProbe(linux), {
    os: 'linux',
    family: 'posix',
    name: 'Linux',
    shell: 'bash',
  })
  const mac = systemFromProbe(
    parseProbeOutput('PISPER_SSH_PROBE\nos=Darwin\nhome=/Users/me\nshell=/bin/zsh\n'),
  )
  assert.deepEqual(mac, { os: 'macos', family: 'posix', name: 'Darwin', shell: 'zsh' })
  const windows = parseProbeOutput(
    'PISPER_SSH_PROBE\r\nos=Windows\r\nversion=10.0.22631.0\r\nhome=C:\\Users\\me\r\nshell=PowerShell 5.1.22621.4391\r\n',
  )
  assert.deepEqual(systemFromProbe(windows, 'cmd'), {
    os: 'windows',
    family: 'windows',
    name: 'Windows 10.0.22631.0',
    shell: 'PowerShell 5.1.22621.4391',
    loginShell: 'cmd',
  })
  assert.deepEqual(parseProbeOutput('PISPER_SSH_PROBE\nos=Linux\nmissing=find\n').missing, ['find'])
  assert.equal(parseProbeOutput('no marker'), null)
  // 读回存盘值：认不出的 os 不用；旧版本没有 system，但探测成功过就是 POSIX（那时只支持 POSIX）。
  assert.equal(normalizeSystem({ os: 'dos', family: 'windows' }), null)
  assert.deepEqual(normalizeSystem(null, { ok: true, platform: 'Darwin' }), {
    os: 'macos',
    family: 'posix',
    name: 'Darwin',
    shell: '',
  })
  assert.equal(normalizeSystem(null, { ok: false, platform: 'Linux' }), null)
  // family 永远由 os 推出，存盘里写的 family 不算数。
  assert.equal(normalizeSystem({ os: 'linux', family: 'windows' }).family, 'posix')
})

test('the model is told which system and shell the machine has', () => {
  const windows = machineSystemPrompt(
    {
      name: 'win',
      system: { os: 'windows', family: 'windows', name: 'Windows 10.0', shell: 'PowerShell 5.1' },
    },
    'C:\\w',
  )
  assert.match(windows, /Windows PowerShell, not bash/)
  assert.match(windows, /Operating system: Windows 10\.0/)
  const cmd = machineSystemPrompt({ name: 'n', system: nodeMachineSystem('win32') }, 'C:\\w')
  assert.match(cmd, /cmd\.exe, not bash/)
  const mac = machineSystemPrompt(
    { name: 'mac', system: { os: 'macos', family: 'posix', name: 'Darwin', shell: 'zsh' } },
    '/Users/me',
  )
  assert.match(mac, /login shell \(zsh\)/)
  assert.match(mac, /BSD tools/)
  const linux = machineSystemPrompt({ name: 'l', system: nodeMachineSystem('linux') }, '/srv')
  assert.ok(!/BSD|PowerShell|cmd\.exe/.test(linux))
  assert.match(machineSystemPrompt({ name: 'x', system: null }, '/srv'), /remote machine "x"/)
  assert.equal(nodeMachineSystem(''), null)
})

// ---------- 这台电脑是 Windows / macOS：找 ssh、传参、连接复用 ----------

test('the ssh client is found the same way on every computer', () => {
  assert.equal(sshExecutable('linux'), 'ssh')
  assert.equal(sshExecutable('darwin'), 'ssh')
  assert.equal(
    sshExecutable(
      'win32',
      { SystemRoot: 'D:\\Win' },
      (path) => path === 'D:\\Win\\System32\\OpenSSH\\ssh.exe',
    ),
    'D:\\Win\\System32\\OpenSSH\\ssh.exe',
  )
  // 可选功能没装：退回 PATH（例如 Git for Windows 的 ssh），都没有时报“找不到 ssh”。
  assert.equal(
    sshExecutable('win32', {}, () => false),
    'ssh',
  )
  assert.deepEqual(parseSshVersion('OpenSSH_for_Windows_8.6p1, LibreSSL 3.4.3'), [8, 6])
  assert.deepEqual(parseSshVersion('OpenSSH_9.6p1 Ubuntu-3ubuntu13.20, OpenSSL 3.0.13'), [9, 6])
  assert.equal(parseSshVersion('Dropbear'), null)
})

test('connection sharing needs a short private folder; Windows never uses it', () => {
  assert.equal(
    muxDirectoryFor('/home/me/.pisper', { platform: 'linux', uid: 1000 }),
    '/home/me/.pisper/ssh-mux',
  )
  // macOS 的 Application Support 太长：socket 路径会超过 104 字节，改用 /tmp 下带 uid 的目录。
  assert.equal(
    muxDirectoryFor('/Users/someone/Library/Application Support/app.pisper.desktop', {
      platform: 'darwin',
      uid: 501,
    }),
    '/tmp/pisper-ssh-501',
  )
  assert.equal(muxDirectoryFor('C:\\Users\\me\\AppData', { platform: 'win32' }), '')
  const machine = { id: 'ssh_aaaaaaaa', name: 'x', target: 'h' }
  assert.equal(new SshExecutor({ machine, muxDirectory: '/m', platform: 'win32' }).muxDirectory, '')
  assert.equal(
    new SshExecutor({ machine, muxDirectory: `/${'x'.repeat(45)}`, platform: 'darwin' })
      .muxDirectory,
    '',
  )
})

// 模拟一次 ssh：记下怎么被调用的，按脚本给出 stdout / stderr / 退出码。
function fakeSsh(script) {
  const calls = []
  const spawnProcess = (file, args, options = {}) => {
    const child = new EventEmitter()
    child.stdout = new PassThrough()
    child.stderr = new PassThrough()
    child.stdin = new PassThrough()
    child.kill = () => {}
    const stdin = []
    child.stdin.on('data', (chunk) => stdin.push(chunk))
    const call = { file, args, options, stdin }
    calls.push(call)
    child.stdin.on('finish', () => {
      const reply = script(call) || {}
      if (reply.stdout) child.stdout.write(reply.stdout)
      if (reply.stderr) child.stderr.write(reply.stderr)
      child.stdout.end()
      child.stderr.end()
      setImmediate(() => {
        child.emit('exit', reply.code ?? 0, null)
        child.emit('close', reply.code ?? 0, null)
      })
    })
    return child
  }
  return { calls, spawnProcess }
}

const passwordMachine = {
  id: 'ssh_password1',
  name: 'pw',
  target: 'me@host',
  port: 2222,
  auth: 'password',
  system: { os: 'linux', family: 'posix', name: 'Linux', shell: 'bash' },
}

test('password sign-in sets the right options and keeps the password out of argv and errors', async () => {
  const args = sshArguments(passwordMachine, { password: true })
  const joined = args.join(' ')
  for (const option of [
    'BatchMode=no',
    'PreferredAuthentications=password,keyboard-interactive',
    'PubkeyAuthentication=no',
    'NumberOfPasswordPrompts=1',
    'StrictHostKeyChecking=accept-new',
    'ForwardAgent=no',
  ])
    assert.ok(joined.includes(option), option)
  assert.ok(!joined.includes('BatchMode=yes'))
  assert.ok(sshArguments(passwordMachine).join(' ').includes('BatchMode=yes'))

  const ssh = fakeSsh(() => ({
    code: 255,
    stderr: 'me@host: Permission denied (password,keyboard-interactive).\r\n',
  }))
  const executor = new SshExecutor({
    machine: passwordMachine,
    platform: 'linux',
    password: () => SECRET,
    spawnProcess: ssh.spawnProcess,
  })
  const startedAt = Date.now()
  const error = await executor
    .operations()
    .read.readFile('/x')
    .catch((caught) => caught)
  assert.equal(error.code, 'auth_failed')
  assert.ok(Date.now() - startedAt < 2_000, 'a wrong password fails at once')
  assert.match(error.message, /wrong user name or password/)
  assert.ok(!error.message.includes(SECRET) && !error.message.includes('S3cr3t'))
  const [call] = ssh.calls
  assert.equal(call.file, 'ssh')
  assert.ok(!call.args.some((arg) => arg.includes('S3cr3t')), 'never on the command line')
  assert.ok(!Buffer.concat(call.stdin).toString().includes('S3cr3t'), 'never sent to the shell')
  const env = call.options.env
  assert.equal(env.SSH_ASKPASS, process.execPath)
  assert.equal(env.SSH_ASKPASS_REQUIRE, 'force')
  assert.equal(env.PISPER_SSH_ASKPASS, '1')
  assert.equal(env.PISPER_SSH_ASKPASS_SECRET, SECRET)
  assert.ok(env.DISPLAY)
  assert.equal(call.options.detached, true, 'no controlling terminal, so old ssh also uses askpass')

  // 密钥登录：没有 askpass 环境，也不另起会话。
  const keySsh = fakeSsh(() => ({ code: 0 }))
  await new SshExecutor({
    machine: { ...passwordMachine, auth: 'key' },
    platform: 'linux',
    spawnProcess: keySsh.spawnProcess,
  })
    .operations()
    .write.mkdir('/x')
  assert.equal(keySsh.calls[0].options.env, undefined)
  assert.equal(keySsh.calls[0].options.detached, undefined)

  // 机器要第二重验证（验证码）：助手拒绝回答，报不支持，不报“密码错”。
  const otp = fakeSsh(() => ({
    code: 255,
    stderr: `${ASKPASS_REFUSED}\nPermission denied (keyboard-interactive).\n`,
  }))
  await assert.rejects(
    new SshExecutor({
      machine: passwordMachine,
      platform: 'linux',
      password: () => SECRET,
      spawnProcess: otp.spawnProcess,
    })
      .operations()
      .read.readFile('/x'),
    { code: 'unsupported_remote' },
  )
  // 选了密码但没存密码：不连，直接说。
  await assert.rejects(
    new SshExecutor({ machine: passwordMachine, spawnProcess: ssh.spawnProcess }).probe(),
    { code: 'auth_failed' },
  )
})

test('password sign-in from Windows needs OpenSSH 8.4 for SSH_ASKPASS_REQUIRE', async () => {
  const make = (version) =>
    fakeSsh((call) =>
      call.args[0] === '-V' ? { stderr: `${version}, LibreSSL 3.4.3\r\n` } : { code: 0 },
    )
  const old = make('OpenSSH_for_Windows_7.7p1')
  await assert.rejects(
    new SshExecutor({
      machine: passwordMachine,
      platform: 'win32',
      sshPath: 'C:\\Windows\\System32\\OpenSSH\\ssh-old.exe',
      password: () => SECRET,
      spawnProcess: old.spawnProcess,
    })
      .operations()
      .write.mkdir('/x'),
    (error) => error.code === 'unsupported_remote' && /8\.4/.test(error.message),
  )
  const current = make('OpenSSH_for_Windows_9.5p1')
  await new SshExecutor({
    machine: passwordMachine,
    platform: 'win32',
    sshPath: 'C:\\Windows\\System32\\OpenSSH\\ssh-new.exe',
    password: () => SECRET,
    spawnProcess: current.spawnProcess,
  })
    .operations()
    .write.mkdir('/x')
  const connect = current.calls.find((call) => call.args[0] !== '-V')
  assert.equal(connect.file, 'C:\\Windows\\System32\\OpenSSH\\ssh-new.exe')
  // Windows：不复用连接，不另起会话（detached 在 Windows 上会弹出控制台），不需要 DISPLAY。
  assert.ok(!connect.args.join(' ').includes('ControlPath'))
  assert.equal(connect.options.detached, false)
  assert.equal(connect.options.env.DISPLAY, process.env.DISPLAY)
})

test('the askpass helper answers password prompts only', async () => {
  assert.equal(askpassAnswer("me@host's password: ", 'pw'), 'pw\n')
  assert.equal(askpassAnswer('(me@host) Password: ', 'pw'), 'pw\n')
  assert.equal(askpassAnswer('密码：', 'pw'), 'pw\n')
  for (const prompt of [
    'Verification code: ',
    'Enter PASSCODE: ',
    'One-time password (OATH) for `me`: ',
    'Duo two-factor login for me',
    'Are you sure you want to continue connecting (yes/no/[fingerprint])? ',
    'Enter PIN for authenticator: ',
  ])
    assert.equal(askpassAnswer(prompt, 'pw'), null, prompt)
  assert.equal(askpassAnswer('Password: ', ''), null)
  assert.equal(
    askpassPrompt(['node', "/cwd/me@host's password: "], '/cwd', 'node'),
    "me@host's password: ",
  )
  assert.equal(askpassPrompt(['/app/pisper', '/app/pisper'], '/x', '/app/pisper'), '')

  // 真的按 ssh 的方式启动一次（开发版：node + NODE_OPTIONS）：提示语是参数，密码从 stdout 出来。
  const env = askpassEnvironment(SECRET, { packaged: false })
  const answered = spawnSync(process.execPath, ["me@host's password: "], { env })
  assert.equal(answered.status, 0, answered.stderr.toString())
  assert.equal(answered.stdout.toString(), `${SECRET}\n`)
  assert.equal(answered.stderr.toString(), '')
  const refused = spawnSync(process.execPath, ['Verification code: '], { env })
  assert.equal(refused.status, 1)
  assert.equal(refused.stdout.toString(), '')
  assert.equal(refused.stderr.toString().trim(), ASKPASS_REFUSED)
  // 打包版不改 NODE_OPTIONS（入口脚本自己认 PISPER_SSH_ASKPASS）。
  assert.equal(
    askpassEnvironment('x', { packaged: true, env: { NODE_OPTIONS: '--a' } }).NODE_OPTIONS,
    undefined,
  )
})

// ---------- 密码的存放 ----------

test('passwords live in their own 0600 file and never in the machine list', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-ssh-secret-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const machinesPath = join(dir, 'pisper-ssh-machines.json')
  const secretsPath = join(dir, 'pisper-ssh-secrets.json')
  // 旧版本存下的机器：没有 auth 字段，按密钥登录读回；探测成功过的按 POSIX。
  await writeFile(
    machinesPath,
    JSON.stringify({
      version: 1,
      machines: [
        {
          id: 'ssh_oldoldold',
          target: 'me@old',
          lastCheck: { ok: true, at: 'x', code: '', platform: 'Linux' },
        },
      ],
    }),
  )
  const service = new SshMachineService({ path: machinesPath, dataDir: dir })
  await service.init()
  const [old] = service.list()
  assert.equal(old.auth, 'key')
  assert.equal(old.passwordSet, false)
  assert.equal(old.system.family, 'posix')

  await assert.rejects(service.add({ target: 'me@new', auth: 'password' }), { code: 'invalid' })
  await assert.rejects(
    service.add({ target: 'me@new', auth: 'password', password: 'a\nb' }),
    (error) => error.code === 'invalid' && !error.message.includes('a\nb'),
  )
  const added = await service.add({ target: 'me@new', auth: 'password', password: SECRET })
  assert.deepEqual([added.auth, added.passwordSet], ['password', true])
  const shown = JSON.stringify([added, service.list()])
  assert.ok(!shown.includes('S3cr3t'), 'list and add responses never carry the password')
  assert.ok(!(await readFile(machinesPath, 'utf8')).includes('S3cr3t'))
  assert.equal(JSON.parse(await readFile(secretsPath, 'utf8')).passwords[added.id], SECRET)
  if (process.platform !== 'win32') assert.equal((await stat(secretsPath)).mode & 0o777, 0o600)
  assert.equal(service.executor(added.id).password(), SECRET)

  // 编辑：不填密码 = 保留；填了 = 换掉；改成密钥 = 删掉。
  const renamed = await service.update(added.id, { name: '改名', auth: 'password' })
  assert.deepEqual([renamed.name, renamed.passwordSet], ['改名', true])
  assert.equal(service.secrets.get(added.id), SECRET)
  await service.update(added.id, { auth: 'password', password: 'second' })
  assert.equal(service.secrets.get(added.id), 'second')
  const cleared = await service.update(added.id, { auth: 'key' })
  assert.deepEqual([cleared.auth, cleared.passwordSet], ['key', false])
  assert.equal(JSON.parse(await readFile(secretsPath, 'utf8')).passwords[added.id], undefined)
  await assert.rejects(service.update(added.id, { auth: 'password' }), { code: 'invalid' })

  // 删除机器连同密码一起删；重启后读回一致。
  await service.update(added.id, { auth: 'password', password: SECRET })
  assert.equal(await service.remove(added.id), true)
  assert.ok(!(await readFile(secretsPath, 'utf8')).includes('S3cr3t'))
  const reloaded = new SshMachineService({ path: machinesPath, dataDir: dir })
  await reloaded.init()
  assert.deepEqual(
    reloaded.list().map((machine) => machine.id),
    ['ssh_oldoldold'],
  )
  assert.equal(normalizeSshPassword(''), '')
  assert.throws(() => normalizeSshPassword('x'.repeat(1025)), { code: 'invalid' })
})

// ---------- Windows 那边的 PowerShell 脚本：有 pwsh / powershell 时真实跑 ----------

const POWERSHELL = [process.env.PISPER_TEST_POWERSHELL, 'pwsh', 'powershell']
  .filter(Boolean)
  .find((name) => spawnSync(name, ['-NoProfile', '-Command', 'exit 0']).status === 0)

// 把 ssh 换成“直接交给对方 shell”：探测那句按 cmd 的展开方式回答，PowerShell 引导命令交给本机的 PowerShell。
function windowsTarget(home, loginShell = 'cmd') {
  return (file, args, options = {}) => {
    const remote = args.at(-1)
    if (remote === SHELL_PROBE_COMMAND) {
      const line =
        loginShell === 'cmd'
          ? '"PISPER_SSH_SHELL_Windows_NT_$PSHOME"\r\n'
          : 'PISPER_SSH_SHELL_%OS%_C:\\Windows\\System32\\WindowsPowerShell\\v1.0\r\n'
      return spawn(process.execPath, ['-e', `process.stdout.write(${JSON.stringify(line)})`])
    }
    assert.equal(remote, POWERSHELL_COMMAND)
    return spawn(POWERSHELL, remote.split(' ').slice(1), {
      ...options,
      cwd: home,
      env: { ...process.env, USERPROFILE: home },
    })
  }
}

const psTest = (name, body) =>
  test(name, { skip: POWERSHELL ? false : '这个环境没有 PowerShell', timeout: 120_000 }, body)

async function windowsExecutor(t, loginShell) {
  const home = await mkdtemp(join(tmpdir(), 'pisper-ps-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  const systems = []
  const executor = new SshExecutor({
    machine: { id: 'ssh_windows1', name: 'win', target: 'me@win' },
    platform: 'linux',
    spawnProcess: windowsTarget(home, loginShell),
    onSystem: (system) => systems.push(system),
  })
  return { home, executor, systems, ops: executor.operations() }
}

psTest('a Windows machine is detected once and remembered', async (t) => {
  for (const loginShell of ['cmd', 'powershell']) {
    const { home, executor, systems } = await windowsExecutor(t, loginShell)
    const probed = await executor.probe()
    assert.equal(probed.os, 'windows')
    assert.equal(probed.home, home)
    assert.match(probed.shell, /^PowerShell \d/)
    assert.equal(systems[0].family, 'windows')
    assert.equal(systems[0].loginShell, loginShell)
  }
})

psTest('files round trip through PowerShell byte for byte', async (t) => {
  const { home, ops, executor } = await windowsExecutor(t)
  const path = join(home, "it's ’odd’ & $(x) %PATH%.txt")
  await ops.write.writeFile(path, '你好\r\n第二行\n')
  assert.equal(await readFile(path, 'utf8'), '你好\r\n第二行\n')
  assert.equal((await ops.read.readFile(path)).toString('utf8'), '你好\r\n第二行\n')
  const bytes = Buffer.from([0, 1, 2, 254, 255, 10, 13, 0x1e])
  await writeFile(join(home, 'bin'), bytes)
  assert.deepEqual(await ops.read.readFile(join(home, 'bin')), bytes)
  await ops.read.access(path)
  await ops.edit.access(path)
  await ops.write.mkdir(join(home, 'a', 'b', 'c'))
  assert.equal(await ops.ls.exists(join(home, 'a', 'b', 'c')), true)
  assert.equal((await ops.ls.stat(join(home, 'a'))).isDirectory(), true)
  assert.equal((await ops.ls.stat(path)).isDirectory(), false)
  assert.deepEqual(
    (await ops.ls.readdir(home)).sort(),
    ['a', 'bin', "it's ’odd’ & $(x) %PATH%.txt"].sort(),
  )
  assert.ok(executor.listCache.size > 0)
  assert.equal(existsSync(join(home, 'x')), false, 'nothing in the name was executed')
  await assert.rejects(ops.read.readFile(join(home, 'nope')), { code: 'not_found' })
  await assert.rejects(ops.read.readFile(home), { code: 'is_directory' })
  await assert.rejects(ops.read.access(join(home, 'nope')), { code: 'not_found' })
  await assert.rejects(ops.ls.readdir(join(home, 'nope')), { code: 'not_found' })
  assert.equal(await ops.ls.exists(join(home, 'nope')), false)
  if (process.platform !== 'win32' && process.getuid?.() !== 0) {
    const locked = join(home, 'locked')
    await writeFile(locked, 'x')
    await chmod(locked, 0o444)
    await assert.rejects(ops.edit.access(locked), { code: 'permission_denied' })
  }
})

psTest('glob and folders follow the same rules on Windows', async (t) => {
  const { home, ops, executor } = await windowsExecutor(t)
  await mkdir(join(home, 'src', 'deep'), { recursive: true })
  await mkdir(join(home, 'node_modules', 'pkg'), { recursive: true })
  await mkdir(join(home, '.git'))
  for (const name of [
    'src/a.ts',
    'src/deep/b.ts',
    'src/c.js',
    'node_modules/pkg/d.ts',
    '.git/e.ts',
    'top.ts',
  ])
    await writeFile(join(home, ...name.split('/')), 'x')
  assert.deepEqual((await ops.find.glob('**/*.ts', home)).sort(), [
    'src/a.ts',
    'src/deep/b.ts',
    'top.ts',
  ])
  assert.deepEqual(await ops.find.glob('**/*.ts', home, { ignore: ['src/**'] }), ['top.ts'])
  assert.deepEqual(await ops.find.glob('node_modules/**/*.ts', home), ['node_modules/pkg/d.ts'])
  assert.equal(await executor.resolveDirectory('~'), home)
  assert.equal(await executor.resolveDirectory(''), home)
  assert.equal(await executor.resolveDirectory('~/src'), join(home, 'src'))
  await assert.rejects(executor.resolveDirectory(join(home, 'nope')), {
    code: 'machine_folder_missing',
  })
})

psTest('commands run in PowerShell with output, exit codes and no input', async (t) => {
  const { home, ops } = await windowsExecutor(t)
  const chunks = []
  const onData = (chunk) => chunks.push(chunk)
  const first = await ops.bash.exec(
    `Get-Location | Select-Object -ExpandProperty Path\nWrite-Output "it's 你好 & | done"\n[Console]::Error.WriteLine('err')\nexit 7`,
    home,
    { onData },
  )
  assert.equal(first.exitCode, 7)
  const text = Buffer.concat(chunks).toString('utf8')
  assert.ok(text.includes(home), text)
  assert.ok(text.includes("it's 你好 & | done"), text)
  assert.ok(text.includes('err'), text)
  assert.ok(!text.includes('\u001e'), 'markers stay out of the output')
  // 原生命令的退出码；读 stdin 立刻到结尾。
  const native = await ops.bash.exec(
    process.platform === 'win32' ? 'cmd /c exit 3' : "sh -c 'exit 3'",
    home,
    { onData: () => {} },
  )
  assert.equal(native.exitCode, 3)
  const startedAt = Date.now()
  const input = await ops.bash.exec('$x = [Console]::In.ReadToEnd(); "read:" + $x.Length', home, {
    onData,
  })
  assert.equal(input.exitCode, 0)
  assert.ok(Date.now() - startedAt < 15_000)
  const thrown = await ops.bash.exec("throw 'boom'", home, { onData: () => {} })
  assert.equal(thrown.exitCode, 1)
  await assert.rejects(
    ops.bash.exec('Write-Output hi', join(home, 'nope'), { onData: () => {} }),
    /Working directory does not exist/,
  )
  // 很长的命令也不受命令行长度限制。
  const long = await ops.bash.exec(`$s = '${'y'.repeat(30_000)}'; $s.Length`, home, {
    onData: (chunk) => chunks.push(chunk),
  })
  assert.equal(long.exitCode, 0)
  assert.ok(Buffer.concat(chunks).toString('utf8').includes('30000'))
})

// ---------- 真实 sshd：密码确实经 askpass 交给了对方，错了立刻失败 ----------

const SSHD = ['/usr/sbin/sshd', '/usr/local/sbin/sshd'].find((path) => existsSync(path))
const skipSshd =
  process.platform === 'win32' || !SSHD || spawnSync('ssh', ['-V']).status !== 0
    ? '这个环境没有 sshd'
    : false

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

test(
  'a real sshd receives the password from the helper and a wrong one fails fast',
  { skip: skipSshd, timeout: 60_000 },
  async (t) => {
    const dir = await mkdtemp(join(tmpdir(), 'pspw-'))
    await chmod(dir, 0o700)
    const port = await freePort()
    await run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', join(dir, 'host_key')])
    await writeFile(
      join(dir, 'sshd_config'),
      [
        `Port ${port}`,
        'ListenAddress 127.0.0.1',
        `HostKey ${join(dir, 'host_key')}`,
        `PidFile ${join(dir, 'sshd.pid')}`,
        'StrictModes no',
        'PubkeyAuthentication no',
        'PasswordAuthentication yes',
        'KbdInteractiveAuthentication no',
        'UsePAM no',
        'LogLevel VERBOSE',
        '',
      ].join('\n'),
    )
    const config = join(dir, 'ssh_config')
    await writeFile(config, `Host *\n  UserKnownHostsFile ${join(dir, 'known_hosts')}\n`)
    let log = ''
    const daemon = spawn(SSHD, ['-D', '-e', '-f', join(dir, 'sshd_config')], {
      stdio: ['ignore', 'ignore', 'pipe'],
    })
    daemon.stderr.on('data', (chunk) => (log += chunk))
    t.after(async () => {
      daemon.kill()
      await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    })
    for (let attempt = 0; attempt < 50 && !log.includes('listening'); attempt += 1)
      await new Promise((resume) => setTimeout(resume, 100))
    const executor = new SshExecutor({
      machine: {
        id: 'ssh_realpass',
        name: 'real',
        target: `${userInfo().username}@127.0.0.1`,
        port,
        auth: 'password',
      },
      extraArguments: ['-F', config],
      password: () => SECRET,
    })
    const startedAt = Date.now()
    const error = await executor.probe().catch((caught) => caught)
    assert.equal(error.code, 'auth_failed', error.message)
    assert.ok(Date.now() - startedAt < 15_000, 'no hanging on a prompt')
    assert.ok(!error.message.includes('S3cr3t'))
    // 对方确实收到了一次密码尝试（askpass 工作了）；Pisper 这一侧的输出里没有密码。
    for (let attempt = 0; attempt < 20 && !/Failed password/.test(log); attempt += 1)
      await new Promise((resume) => setTimeout(resume, 100))
    assert.match(log, /Failed password for/)
    assert.ok(!log.includes('S3cr3t'))
    assert.ok(existsSync(join(dir, 'known_hosts')), 'the host key is still recorded first time')
  },
)
