import assert from 'node:assert/strict'
import { fork, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { access, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
import { criticalRuntimeEntries, inspectCriticalFiles } from '../../scripts/sea-runtime.mjs'
import {
  RUNTIME_BUNDLE_SCHEMA,
  RUNTIME_EXTERNAL_PACKAGES,
  bundleRuntime,
} from '../../scripts/runtime-bundle.mjs'

async function createFile(root, relativePath, contents) {
  const path = join(root, ...relativePath.split('/'))
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, contents)
}

async function exists(path) {
  try {
    await access(path)
    return true
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

async function checkWorkerIpc(workerPath) {
  const child = fork(workerPath, ['--pisper-speech-worker'], {
    execArgv: [],
    env: {},
    serialization: 'advanced',
    stdio: ['ignore', 'ignore', 'pipe', 'ipc'],
  })
  let stderr = ''
  let reply = null
  let failure = null
  child.stderr.on('data', (chunk) => {
    stderr += String(chunk)
  })
  const closed = new Promise((resolveClose) => child.once('close', resolveClose))
  const timeout = setTimeout(() => {
    failure = new Error('Bundled speech worker timed out')
    child.kill('SIGKILL')
  }, 15_000)
  child.once('error', (error) => {
    failure = error
  })
  child.once('message', (message) => {
    reply = message
    child.send({ method: 'shutdown' }, (error) => {
      if (error) {
        failure = error
        child.kill('SIGKILL')
      }
    })
  })
  child.send({ id: 'bundle-smoke', method: 'smoke' }, (error) => {
    if (error) {
      failure = error
      child.kill('SIGKILL')
    }
  })
  const code = await closed
  clearTimeout(timeout)
  assert.ifError(failure)
  assert.equal(code, 0, stderr)
  assert.deepEqual(reply, { id: 'bundle-smoke', ok: false, error: { code: 'config' } })
  assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' })
}

test('Runtime bundle preserves host entries and only declares external package roots', async () => {
  const runtimeDir = await mkdtemp(join(await realpath(tmpdir()), 'pisper-runtime-bundle-'))
  const dependencies = Object.fromEntries(RUNTIME_EXTERNAL_PACKAGES.map((name) => [name, '1.0.0']))
  try {
    const speechSources = [
      'runtime/workers/speech-inference-worker.mjs',
      'runtime/services/speech-recognition-service.mjs',
      'runtime/services/speech-engine-service.mjs',
      'shared/speech-terms.mjs',
      'shared/speech-resources/xasr-bpe.vocab',
      'shared/speech-model-catalog.json',
      'shared/speech-resource-notices.json',
    ]
    await Promise.all(
      speechSources.map(async (path) => {
        await createFile(
          runtimeDir,
          path,
          await readFile(new URL(`../../${path}`, import.meta.url)),
        )
      }),
    )
    await Promise.all([
      createFile(
        runtimeDir,
        'package.json',
        `${JSON.stringify({ name: 'fixture', type: 'module', dependencies })}\n`,
      ),
      createFile(
        runtimeDir,
        'runtime/sidecar.mjs',
        "import { value } from '../shared/value.mjs'\nexport { SpeechEngineService } from './services/speech-engine-service.mjs'\nglobalThis.__bundleSidecar = value\n",
      ),
      createFile(
        runtimeDir,
        'runtime/mobile-embedded.mjs',
        "import { value } from '../shared/value.mjs'\nglobalThis.__bundleMobile = value\n",
      ),
      createFile(
        runtimeDir,
        'runtime/plugins/local-plugin-worker.mjs',
        "export const worker = 'fixture'\n",
      ),
      createFile(
        runtimeDir,
        'runtime/workers/team-workflow-worker.mjs',
        "export const teamWorker = 'fixture'\n",
      ),
      createFile(runtimeDir, 'shared/value.mjs', "export const value = 'bundled'\n"),
      createFile(runtimeDir, 'shared/unreferenced.onnx', 'not a packaged model'),
      createFile(runtimeDir, 'runtime/unreferenced.onnx', 'not a packaged model'),
    ])

    const manifest = await bundleRuntime({ runtimeDir })
    const stagedPackage = JSON.parse(await readFile(join(runtimeDir, 'package.json'), 'utf8'))

    assert.equal(manifest.schema, RUNTIME_BUNDLE_SCHEMA)
    assert.deepEqual(manifest.entries, [
      'runtime/sidecar.mjs',
      'runtime/mobile-embedded.mjs',
      'runtime/workers/speech-inference-worker.mjs',
    ])
    assert.deepEqual(Object.keys(stagedPackage.dependencies), [...RUNTIME_EXTERNAL_PACKAGES])
    assert.equal(await exists(join(runtimeDir, 'shared', 'value.mjs')), false)
    assert.equal(await exists(join(runtimeDir, 'shared', 'unreferenced.onnx')), false)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'unreferenced.onnx')), false)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'services')), false)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'speech-inference-worker.mjs')), false)
    const resourcePath = 'shared/speech-resources/xasr-bpe.vocab'
    const bpe = await readFile(join(runtimeDir, resourcePath))
    assert.equal(bpe.length, 61562)
    assert.equal(
      createHash('sha256').update(bpe).digest('hex'),
      '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5',
    )
    const retained = [
      'shared/speech-model-catalog.json',
      'shared/speech-resource-notices.json',
      resourcePath,
    ]
    const expectedResources = []
    for (const path of retained) {
      const source = await readFile(new URL(`../../${path}`, import.meta.url))
      assert.deepEqual(await readFile(join(runtimeDir, path)), source)
      expectedResources.push({ path, bytes: source.length })
    }
    assert.deepEqual(
      manifest.files.filter((file) => file.path.startsWith('shared/')),
      expectedResources.sort((left, right) => left.path.localeCompare(right.path)),
    )
    assert.equal(manifest.outputFileCount, manifest.files.length)
    assert.equal(
      manifest.outputBytes,
      manifest.files.reduce((sum, file) => sum + file.bytes, 0),
    )
    assert.equal(await exists(join(runtimeDir, 'runtime', 'sidecar.mjs')), true)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'mobile-embedded.mjs')), true)
    assert.equal(await exists(join(runtimeDir, 'THIRD_PARTY_LICENSES.txt')), true)
    assert.equal(
      await exists(join(runtimeDir, 'runtime', 'plugins', 'local-plugin-worker.mjs')),
      true,
    )
    assert.equal(
      await exists(join(runtimeDir, 'runtime', 'workers', 'team-workflow-worker.mjs')),
      true,
    )
    assert.ok(manifest.inputFileCount >= 3)
    assert.ok(manifest.outputFileCount >= 3)

    const { SpeechEngineService } = await import(
      pathToFileURL(join(runtimeDir, 'runtime', 'sidecar.mjs')).href
    )
    await import(pathToFileURL(join(runtimeDir, 'runtime', 'mobile-embedded.mjs')).href)
    assert.equal(globalThis.__bundleSidecar, 'bundled')
    assert.equal(globalThis.__bundleMobile, 'bundled')

    const workerPath = join(runtimeDir, 'runtime', 'workers', 'speech-inference-worker.mjs')
    const engine = new SpeechEngineService({ catalog: { models: [] }, modelDownloads: {} })
    assert.equal(engine.workerUrl.href, pathToFileURL(workerPath).href)
    await engine.dispose()
    const { createSpeechInferenceHandler } = await import(pathToFileURL(workerPath).href)
    // 校验错误必须来自独立打包的 VITS worker，而不是缺失源码或依赖。
    await assert.rejects(
      createSpeechInferenceHandler()('init', {
        kind: 'tts',
        modelDir: join(runtimeDir, 'models'),
        model: {
          id: 'vits-melo-tts-zh_en',
          engine: 'vits',
          config: {
            model: '../outside.onnx',
            tokens: 'tokens.txt',
            lexicon: 'lexicon.txt',
            dictDir: 'dict',
          },
          files: [],
        },
      }),
      { code: 'config' },
    )
    await checkWorkerIpc(workerPath)
    const noIpc = spawnSync(process.execPath, [workerPath, '--pisper-speech-worker'], {
      env: {},
      encoding: 'utf8',
      timeout: 15_000,
    })
    assert.ifError(noIpc.error)
    assert.equal(noIpc.status, 1)
    assert.match(noIpc.stderr, /Speech inference process stopped unexpectedly/)

    const criticalSpeechPaths = ['runtime/workers/speech-inference-worker.mjs', ...retained]
    const entries = criticalRuntimeEntries().filter((entry) =>
      criticalSpeechPaths.includes(entry.path),
    )
    assert.equal(entries.length, 4)
    assert.ok((await inspectCriticalFiles(runtimeDir, entries)).every((entry) => entry.exists))
    for (const entry of entries) {
      await rm(join(runtimeDir, entry.path))
      const [missing] = await inspectCriticalFiles(runtimeDir, [entry])
      assert.equal(missing.exists, false)
    }
  } finally {
    delete globalThis.__bundleSidecar
    delete globalThis.__bundleMobile
    await rm(runtimeDir, { recursive: true, force: true })
  }
})

test('Runtime bundle rejects a missing external production dependency', async () => {
  const runtimeDir = await mkdtemp(join(await realpath(tmpdir()), 'pisper-runtime-bundle-missing-'))
  try {
    await Promise.all([
      createFile(
        runtimeDir,
        'package.json',
        `${JSON.stringify({ name: 'fixture', type: 'module', dependencies: {} })}\n`,
      ),
      createFile(runtimeDir, 'runtime/sidecar.mjs', 'export {}\n'),
      createFile(runtimeDir, 'runtime/mobile-embedded.mjs', 'export {}\n'),
      createFile(runtimeDir, 'runtime/plugins/local-plugin-worker.mjs', 'export {}\n'),
      createFile(runtimeDir, 'shared/value.mjs', 'export {}\n'),
    ])

    await assert.rejects(
      bundleRuntime({ runtimeDir }),
      /Runtime external package is not a production dependency/,
    )
  } finally {
    await rm(runtimeDir, { recursive: true, force: true })
  }
})
