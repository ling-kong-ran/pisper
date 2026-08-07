import assert from 'node:assert/strict'
import test from 'node:test'
import { createPisperBashTool, hostCommandEnvironment } from '../tools/host-bash.mjs'

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

test('Pisper bash exposes only command and timeout without sandbox escalation flags', async () => {
  const tool = await createPisperBashTool(process.cwd())
  const properties = tool.parameters.properties

  assert.deepEqual(Object.keys(properties).sort(), ['command', 'timeout'])
  assert.equal('sandbox_permissions' in properties, false)
  assert.match(tool.description, /operating-system user/)
  assert.match(tool.description, /host files and networks/)
  assert.doesNotMatch(tool.description, /sandbox/i)
})
