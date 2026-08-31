import { existsSync } from 'node:fs'
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createPisperBashTool,
  hostCommandEnvironment,
  selectHostShell,
  windowsPowerShellArguments,
  windowsPowerShellExecutable,
} from '../tools/host-bash.mjs'

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
