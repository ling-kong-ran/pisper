import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { createPisperRuntime } from '../app-runtime.mjs'

class TestRuntimeService {
  async init() {}
  async dispose() {}
}

test('deferred runtime initialization listens first and fails API requests cleanly', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-app-runtime-'))
  const dataDir = join(root, 'agent')
  const stages = []
  let app = null
  await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.0.0-test' }))

  try {
    app = await createPisperRuntime({
      root,
      runtimeCwd: root,
      dataDir,
      production: true,
      port: 0,
      deferRuntimeInitialization: true,
      startupObserver: (stage) => stages.push(stage),
      runtimeModuleLoader: async () => {
        throw new Error('injected initialization failure')
      },
    })

    await assert.rejects(app.initialized, /injected initialization failure/)
    const response = await fetch(`${app.url}/api/config`)
    assert.equal(response.status, 503)
    assert.deepEqual(await response.json(), {
      error: 'Pisper runtime initialization failed.',
    })
    assert.equal(stages[0], 'http-listening')
    assert.equal(stages[1], 'runtime-modules-loading')
  } finally {
    await app?.close()
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
})

test('request rejection returns 500 without terminating the HTTP server', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-app-runtime-request-'))
  const dataDir = join(root, 'agent')
  let app = null
  let requests = 0
  await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.0.0-test' }))

  try {
    app = await createPisperRuntime({
      root,
      runtimeCwd: root,
      dataDir,
      production: true,
      port: 0,
      runtimeModuleLoader: async () => ({ AgentRuntimeService: TestRuntimeService }),
      apiHandlerModuleLoader: async () => ({
        createApiHandler: () => async (_req, res) => {
          requests += 1
          if (requests === 1) throw new Error('injected request failure')
          res.writeHead(204)
          res.end()
          return true
        },
      }),
    })

    const failed = await fetch(`${app.url}/api/injected-failure`)
    assert.equal(failed.status, 500)
    assert.deepEqual(await failed.json(), { error: 'Pisper request failed.' })

    const recovered = await fetch(`${app.url}/api/recovered`)
    assert.equal(recovered.status, 204)
    assert.equal(requests, 2)
  } finally {
    await app?.close()
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
})
