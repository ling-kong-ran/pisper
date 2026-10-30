import assert from 'node:assert/strict'
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'

import { createPisperRuntime, resolveViteCacheDir } from '../app-runtime.mjs'

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

test('dev Vite cache dir is optional, absolute only and validated before anything starts', async () => {
  // 不传时为 null：createViteServer 收到的配置与以前完全相同，`npm run dev` 仍用默认 node_modules/.vite。
  assert.equal(resolveViteCacheDir(undefined), null)
  assert.equal(resolveViteCacheDir(null), null)
  const absolute = join(tmpdir(), 'pisper-vite-cache', '..', 'pisper-vite-cache')
  assert.equal(resolveViteCacheDir(absolute), resolve(absolute))
  for (const invalid of ['', '   ', 'relative/vite-cache', './vite-cache', 42, {}, `/tmp/a\0b`])
    assert.throws(() => resolveViteCacheDir(invalid), /viteCacheDir must be an absolute/)

  // 非法值在建任何服务、监听端口之前就被拒绝。
  const root = await mkdtemp(join(tmpdir(), 'pisper-app-runtime-vite-cache-'))
  try {
    await assert.rejects(
      createPisperRuntime({
        root,
        runtimeCwd: root,
        dataDir: join(root, 'agent'),
        production: false,
        port: 0,
        viteCacheDir: 'node_modules/.vite',
        runtimeModuleLoader: async () => {
          throw new Error('runtime must not load for an invalid viteCacheDir')
        },
      }),
      /viteCacheDir must be an absolute/,
    )
    assert.deepEqual(await readdir(root), [])
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
})

test('smokes that start a dev Vite server use their own dependency cache', async () => {
  // 负责人的 dev 服务占着默认 node_modules/.vite；冒烟起的开发 Runtime 或直接起的 Vite 服务若也用它，
  // 并发预构建依赖会互相改写缓存。生产模式（production: true）不起 Vite，不受此约束。
  const scripts = new URL('../../scripts/', import.meta.url)
  const files = [
    ...(await readdir(scripts)).filter((name) => name.endsWith('.mjs')),
    ...(await readdir(new URL('fixtures/', scripts)))
      .filter((name) => name.endsWith('.mjs'))
      .map((name) => `fixtures/${name}`),
  ]
  const devRuntimes = []
  const viteServers = []
  for (const name of files) {
    const source = await readFile(new URL(name, scripts), 'utf8')
    for (const match of source.matchAll(/createPisperRuntime\(\{([\s\S]*?)\n\s*\}\)/g)) {
      if (/production:\s*true\b/.test(match[1])) continue
      devRuntimes.push(name)
      assert.match(
        match[1],
        /viteCacheDir:/,
        `${name} starts a dev Runtime on the shared Vite cache`,
      )
    }
    if (/import \{[^}]*\bcreateServer\b[^}]*\} from 'vite'/.test(source)) {
      viteServers.push(name)
      assert.match(source, /\bcacheDir\b/, `${name} starts Vite on the shared dependency cache`)
    }
  }
  // 扫描确实覆盖到了已知的开发 Runtime 与 Vite 冒烟，避免正则失配时空转。
  for (const name of [
    'smoke-pisper-first-run-checks.mjs',
    'smoke-workflows-ui.mjs',
    'smoke-game-asset-workbench.mjs',
  ])
    assert.ok(devRuntimes.includes(name), name)
  for (const name of ['smoke-frontend-workbench.mjs', 'smoke-replica-shell.mjs'])
    assert.ok(viteServers.includes(name), name)
})
