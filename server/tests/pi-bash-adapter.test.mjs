import assert from 'node:assert/strict'
import { resolve } from 'node:path'
import test from 'node:test'
import {
  createSandboxBashOperations,
  sandboxExecutionEnvironment,
  sandboxLogicalCwd,
} from '../sandbox/pi-bash-adapter.mjs'

test('sandbox bash adapter maps cwd, environment, timeout, and output', async () => {
  const workspace = process.cwd()
  const calls = []
  const operations = createSandboxBashOperations({
    workspace,
    getSandbox: async () => ({
      async exec(input) {
        calls.push(input)
        input.onOutput({ bytes: Buffer.from('chunk') })
        return { exitCode: 7, signal: null, usage: {} }
      },
    }),
  })
  const output = []
  assert.deepEqual(
    await operations.exec('exit 7', workspace, {
      timeout: 2,
      env: { CI: '1', OPENAI_API_KEY: 'secret' },
      onData: (chunk) => output.push(chunk.toString('utf8')),
    }),
    { exitCode: 7 },
  )
  assert.equal(calls[0].command.script, 'exit 7')
  assert.deepEqual(calls[0].cwd, { mount: 'workspace', path: '.' })
  assert.deepEqual(calls[0].env, { CI: '1' })
  assert.deepEqual(calls[0].limits, { wallTimeMs: 2_000 })
  assert.deepEqual(output, ['chunk'])
})

test('sandbox bash adapter rejects cwd escape and sensitive environment', () => {
  const workspace = resolve(process.cwd(), 'workspace')
  assert.throws(() => sandboxLogicalCwd(workspace, resolve(workspace, '..')), /outside/)
  assert.deepEqual(
    sandboxExecutionEnvironment({ TERM: 'xterm', GITHUB_TOKEN: 'secret', CUSTOM: 'value' }),
    { TERM: 'xterm' },
  )
})

test('sandbox bash adapter preserves Pi abort and timeout error contracts', async () => {
  const workspace = process.cwd()
  const operationFor = (signal) =>
    createSandboxBashOperations({
      workspace,
      getSandbox: async () => ({
        exec: async () => ({ exitCode: null, signal, usage: {} }),
      }),
    })
  await assert.rejects(operationFor('timeout').exec('wait', workspace, { timeout: 3 }), /timeout:3/)
  await assert.rejects(operationFor('terminate').exec('wait', workspace, {}), /aborted/)
})
