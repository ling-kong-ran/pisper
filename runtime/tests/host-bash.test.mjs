import { existsSync } from 'node:fs'
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createPisperBashTool,
  createWindowsSystemShellOperations,
  hostCommandEnvironment,
  selectHostShell,
  windowsPowerShellArguments,
  windowsPowerShellExecutable,
} from '../tools/host-bash.mjs'

function processExists(pid) {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    if (error.code === 'ESRCH') return false
    throw error
  }
}

for (const stop of ['abort', 'timeout']) {
  test(
    `PowerShell fallback ${stop} terminates interpreter descendants`,
    { timeout: 15_000 },
    async (t) => {
      const directory = await mkdtemp(join(tmpdir(), 'pisper-shell-tree-'))
      const environment =
        process.platform === 'win32' ? process.env : { SystemRoot: join(directory, 'system') }
      // 非 Windows 使用相同父子关系的可执行夹具；Windows 真正经过系统 PowerShell。
      const fixture =
        process.platform === 'win32'
          ? join(directory, 'interpreter.cjs')
          : windowsPowerShellExecutable(environment)
      const pids = []
      let execution
      const abort = new AbortController()
      t.after(async () => {
        abort.abort()
        await execution?.catch(() => {})
        for (const pid of pids) {
          if (processExists(pid)) process.kill(pid, 'SIGKILL')
        }
        await rm(directory, { recursive: true, force: true })
      })
      await writeFile(
        fixture,
        `#!${process.execPath}
const { spawn } = require('node:child_process')
const child = spawn(process.execPath, ['-e', "process.send('ready'); setInterval(() => {}, 1000)"], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] })
child.on('message', () => process.stdout.write(JSON.stringify({ launcher: process.pid, worker: child.pid }) + '\\n'))
setInterval(() => {}, 1000)
`,
      )
      if (process.platform !== 'win32') await chmod(fixture, 0o700)
      const operations = await createWindowsSystemShellOperations(environment)
      const ready = Promise.withResolvers()
      let output = ''
      const quote = (value) => `'${value.replaceAll("'", "''")}'`
      execution = operations.exec(`& ${quote(process.execPath)} ${quote(fixture)}`, directory, {
        signal: abort.signal,
        timeout: stop === 'timeout' ? 2 : 10,
        env: process.env,
        onData: (chunk) => {
          output += chunk.toString()
          if (!output.includes('\n') || pids.length) return
          const result = JSON.parse(output.trim())
          pids.push(result.launcher, result.worker)
          ready.resolve()
        },
      })
      const rejected = assert.rejects(execution, stop === 'abort' ? /aborted/ : /timeout:2/)
      await Promise.race([ready.promise, execution])
      assert.equal(pids.length, 2)
      if (stop === 'abort') abort.abort()
      await rejected
      // 等操作系统回收已终止的进程，以状态为条件轮询，避免固定延时掩盖泄漏。
      const deadline = Date.now() + 3000
      while (pids.some(processExists) && Date.now() < deadline) await delay(20)
      assert.deepEqual(pids.filter(processExists), [], 'interpreter processes outlived the tool')
    },
  )
}

test('host shell environment removes credentials and shell injection variables', () => {
  const environment = hostCommandEnvironment({
    PATH: 'tools',
    LANG: 'C.UTF-8',
    OPENAI_API_KEY: 'secret',
    CUSTOM_API_KEY: 'secret',
    SERVICE_AUTH_TOKEN: 'secret',
    DATABASE_URL: 'postgres://secret',
    BASH_ENV: '/tmp/inject.sh',
    PROMPT_COMMAND: 'inject',
  })

  assert.deepEqual(environment, { PATH: 'tools', LANG: 'C.UTF-8' })
})

test('Windows falls back to the system PowerShell when Bash is unavailable', async () => {
  const fallback = await selectHostShell('win32', async () => {
    throw new Error('No bash shell found')
  })

  assert.deepEqual(fallback, { fallback: true })
  assert.equal(
    windowsPowerShellExecutable({ SystemRoot: 'C:\\Windows' }),
    'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe',
  )
  assert.deepEqual(windowsPowerShellArguments('Get-Location'), [
    '-NoLogo',
    '-NoProfile',
    '-NonInteractive',
    '-Command',
    'Get-Location',
  ])
})

test('Windows keeps Bash when a Bash executable is available', async () => {
  const selected = await selectHostShell('win32', async () => ({ shell: 'bash.exe' }))
  assert.deepEqual(selected, { fallback: false })
})

test('Pisper bash describes the PowerShell fallback instead of claiming Bash', async () => {
  const tool = await createPisperBashTool(process.cwd(), {
    platform: 'win32',
    shellConfig: async () => {
      throw new Error('No bash shell found')
    },
    environment: { SystemRoot: 'C:\\Windows' },
  })

  assert.match(tool.description, /system Windows PowerShell \(powershell\.exe\)/)
  assert.doesNotMatch(tool.description, /Execute a bash command/)
  assert.equal(tool.promptSnippet, 'Execute commands with Windows PowerShell')
})

test(
  'Windows PowerShell fallback executes PowerShell commands',
  { skip: process.platform !== 'win32' || !existsSync(windowsPowerShellExecutable()) },
  async () => {
    const tool = await createPisperBashTool(process.cwd(), {
      platform: 'win32',
      shellConfig: async () => {
        throw new Error('No bash shell found')
      },
    })
    const result = await tool.execute('powershell-fallback-test', {
      command: 'Write-Output "powershell fallback ok"',
    })

    assert.match(result.content[0].text, /powershell fallback ok/)
  },
)

test('Pisper bash exposes only command and timeout without sandbox escalation flags', async () => {
  const tool = await createPisperBashTool(process.cwd())
  const properties = tool.parameters.properties

  assert.deepEqual(Object.keys(properties).sort(), ['command', 'timeout'])
  assert.equal('sandbox_permissions' in properties, false)
  assert.match(tool.description, /operating-system user/)
  assert.match(tool.description, /host files and networks/)
  assert.doesNotMatch(tool.description, /sandbox/i)
})
