import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SandboxService, sandboxDaemonEnvironment } from '../sandbox/sandbox-service.mjs'

function fakeClientRuntime(events) {
  return async (options) => {
    events.push(['spawn', options])
    return {
      async createSandbox(input) {
        events.push(['create', input])
        return {
          id: `sandbox-${events.length}`,
          status: 'limited',
          async exec(execution) {
            execution.onOutput?.({ stream: 'stdout', bytes: Buffer.from('sandboxed') })
            return { exitCode: 0, signal: null, usage: {} }
          },
          async close() {
            events.push(['closeSandbox'])
          },
        }
      },
      async close() {
        events.push(['closeClient'])
      },
    }
  }
}

test('sandbox daemon environment removes credential-like values', () => {
  assert.deepEqual(
    sandboxDaemonEnvironment({ PATH: 'tools', OPENAI_API_KEY: 'secret', SERVICE_AUTH_TOKEN: 'x' }),
    { PATH: 'tools' },
  )
})

test('sandbox service lazily reuses a context and closes it', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-sandbox-service-'))
  const events = []
  const service = new SandboxService({
    dataDir: join(directory, 'data'),
    executable: process.execPath,
    clientFactory: fakeClientRuntime(events),
  })
  try {
    const operations = service.createBashOperations({ contextId: 'session:one', cwd: directory })
    const output = []
    assert.deepEqual(
      await operations.exec('echo test', directory, {
        onData: (chunk) => output.push(chunk.toString('utf8')),
        env: { CI: '1', OPENAI_API_KEY: 'secret' },
      }),
      { exitCode: 0 },
    )
    await operations.exec('echo again', directory, { onData: () => {} })
    assert.deepEqual(output, ['sandboxed'])
    assert.equal(events.filter(([type]) => type === 'spawn').length, 1)
    assert.equal(events.filter(([type]) => type === 'create').length, 1)
    assert.deepEqual(events.find(([type]) => type === 'create')[1].policy.network, { mode: 'deny' })
    assert.equal(await service.closeContext('session:one'), true)
  } finally {
    await service.close()
    await rm(directory, { recursive: true, force: true })
  }
})

test('missing sandbox binary fails closed before client launch', async () => {
  let launched = false
  const service = new SandboxService({
    dataDir: process.cwd(),
    executable: join(process.cwd(), 'missing-agent-sandboxd'),
    clientFactory: async () => {
      launched = true
    },
  })
  await assert.rejects(
    service.ensureContext({ contextId: 'session:missing', cwd: process.cwd() }),
    /host fallback is disabled/i,
  )
  assert.equal(launched, false)
})
