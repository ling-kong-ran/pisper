import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { access, mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { smokeSpeechNative } from '../../scripts/smoke-sea.mjs'

const bootstrap = fileURLToPath(new URL('../../scripts/sea-bootstrap.cjs', import.meta.url))
const platform = process.platform === 'win32' ? 'win' : process.platform
const nativeName = `sherpa-onnx-${platform}-${process.arch}`

async function file(path, source) {
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, source)
}

async function fixture(t, { native = true, wrapper = true, simulateSea = true } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-speech-native-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const runtimeDir = join(root, 'stage')
  const modules = join(runtimeDir, 'node_modules')
  await mkdir(modules, { recursive: true })
  const wrapperPath = join(modules, 'sherpa-onnx-node', 'index.js')
  const addonPath = join(modules, nativeName, 'sherpa-onnx.node')
  if (wrapper) {
    await file(
      wrapperPath,
      `require('../${nativeName}/sherpa-onnx.node')\nmodule.exports = { OnlineRecognizer() { throw new Error('Must not initialize a model') }, OfflineTts() { throw new Error('Must not initialize a model') } }\n`,
    )
  }
  if (native) {
    await file(
      addonPath,
      "module.exports = { createOnlineRecognizer() { throw new Error('Must not initialize a model') }, createOfflineTts() { throw new Error('Must not initialize a model') } }\n",
    )
  }
  const driver = join(root, 'driver.cjs')
  // 定向测试模拟 SEA 标志和原生加载器；真实 SEA/动态链接由产物 smoke 验证。
  await file(
    driver,
    `${simulateSea ? "require('node:sea').isSea = () => true\n" : ''}require('node:module').syncBuiltinESMExports()\nrequire.extensions['.node'] = require.extensions['.js']\nrequire(${JSON.stringify(bootstrap)})\n`,
  )
  const probeRoots = []
  const smoke = (options = {}) =>
    smokeSpeechNative({
      executablePath: process.execPath,
      runtimeDir,
      spawnProcess(executable, args, spawnOptions) {
        assert.equal(executable, process.execPath)
        assert.deepEqual(args, [])
        assert.equal(spawnOptions.cwd, runtimeDir)
        assert.deepEqual(Object.keys(spawnOptions.env), ['PISPER_APP_ROOT'])
        probeRoots.push(spawnOptions.env.PISPER_APP_ROOT)
        return spawn(executable, [driver], spawnOptions)
      },
      ...options,
    })
  return { root, runtimeDir, modules, wrapperPath, addonPath, probeRoots, smoke }
}

async function assertProbeRemoved(input) {
  assert.equal(input.probeRoots.length, 1)
  await assert.rejects(access(input.probeRoots[0]), { code: 'ENOENT' })
}

test('speech native smoke loads staged wrapper and addon without initializing models', async (t) => {
  const input = await fixture(t)
  await input.smoke()
  await assertProbeRemoved(input)
})

test('speech native smoke fails when the platform addon is missing', async (t) => {
  const input = await fixture(t, { native: false })
  await assert.rejects(input.smoke(), /Cannot find module.*sherpa-onnx-/s)
  await assertProbeRemoved(input)
})

test('speech native smoke preserves native loader errors', async (t) => {
  const input = await fixture(t)
  await writeFile(
    input.addonPath,
    "throw new Error('dlopen: Library not loaded: libonnxruntime')\n",
  )
  await assert.rejects(input.smoke(), /dlopen: Library not loaded: libonnxruntime/)
  await assertProbeRemoved(input)
})

for (const [target, source, missing] of [
  [
    'addonPath',
    'module.exports = { createOnlineRecognizer() {}, createOfflineTts: 1 }',
    'createOfflineTts',
  ],
  [
    'addonPath',
    'module.exports = { createOnlineRecognizer: 1, createOfflineTts() {} }',
    'createOnlineRecognizer',
  ],
  ['wrapperPath', 'module.exports = { OnlineRecognizer() {}, OfflineTts: 1 }', 'OfflineTts'],
  ['wrapperPath', 'module.exports = { OnlineRecognizer: 1, OfflineTts() {} }', 'OnlineRecognizer'],
]) {
  test(`speech native smoke rejects invalid ${missing} exports`, async (t) => {
    const input = await fixture(t)
    await writeFile(input[target], source)
    await assert.rejects(input.smoke(), new RegExp(`not a function: ${missing}`))
  })
}

test('speech native smoke rejects a wrapper resolved from parent node_modules', async (t) => {
  const input = await fixture(t, { wrapper: false })
  await file(
    join(input.root, 'node_modules', 'sherpa-onnx-node', 'index.js'),
    'throw new Error("Parent wrapper must never execute")',
  )
  await assert.rejects(input.smoke(), /Speech dependency resolved outside staged runtime/)
})

test('speech native smoke rejects a platform addon resolved from parent node_modules', async (t) => {
  const input = await fixture(t, { native: false })
  await file(
    join(input.root, 'node_modules', nativeName, 'sherpa-onnx.node'),
    'throw new Error("Parent addon must never execute")',
  )
  await assert.rejects(input.smoke(), /Speech dependency resolved outside staged runtime/)
})

test('speech native smoke rejects a symlink escaping the staged runtime', async (t) => {
  const input = await fixture(t, { native: false })
  const external = join(input.root, 'external')
  await file(
    join(external, 'sherpa-onnx.node'),
    'throw new Error("External addon must never execute")',
  )
  await symlink(
    external,
    join(input.modules, nativeName),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(input.smoke(), /Staged runtime symlink resolves outside staged runtime/)
})

for (const path of [
  'runtime/speech-model/encoder.onnx',
  'shared/model.ort',
  'node_modules/package/weights.gguf',
  'weights.safetensors',
  'runtime/other/model.tflite',
  'node_modules/package/MODEL.ONNX',
]) {
  test(`speech native smoke rejects staged model ${path} before starting SEA`, async (t) => {
    const input = await fixture(t)
    await file(join(input.runtimeDir, path), '')
    await assert.rejects(input.smoke(), /Model weights must not be packaged in staged runtime/)
    assert.deepEqual(input.probeRoots, [])
  })
}

test('speech native smoke permits speech metadata, vocabulary and native libraries', async (t) => {
  const input = await fixture(t)
  for (const path of [
    'shared/speech-model-catalog.json',
    'shared/speech-resources/xasr-bpe.vocab',
    'runtime/tokens.txt',
    'node_modules/package/libonnxruntime.so',
    'node_modules/package/libonnxruntime.dylib',
    'node_modules/package/onnxruntime.dll',
  ]) {
    await file(join(input.runtimeDir, path), '')
  }
  await input.smoke()
})

for (const [name, source] of [
  ['missing success marker', 'process.exit(0)'],
  ['unexpected stdout', "process.stdout.write('unexpected output'); module.exports = {}"],
  ['nonzero exit', "process.stdout.write('PISPER_SEA_SPEECH_NATIVE_OK\\n'); process.exit(1)"],
]) {
  test(`speech native smoke rejects ${name}`, async (t) => {
    const input = await fixture(t)
    await writeFile(input.addonPath, source)
    await assert.rejects(input.smoke(), /SEA speech native smoke failed/)
    await assertProbeRemoved(input)
  })
}

test('speech native smoke rejects a regular Node process', async (t) => {
  const input = await fixture(t, { simulateSea: false })
  await assert.rejects(input.smoke(), /must run inside the SEA executable/)
})

test('speech native smoke terminates a stuck child and cleans its probe', async (t) => {
  const input = await fixture(t)
  await writeFile(input.addonPath, 'setInterval(() => {}, 1000); while (true) {}')
  await assert.rejects(input.smoke({ timeoutMs: 200 }), (error) => {
    assert.match(error.cause?.message || '', /timed out/)
    return true
  })
  await assertProbeRemoved(input)
})

test('speech native smoke fails and cleans up when the executable cannot start', async (t) => {
  const input = await fixture(t)
  await assert.rejects(
    input.smoke({ executablePath: join(input.root, 'missing-executable'), spawnProcess: spawn }),
    (error) => error.cause?.code === 'ENOENT',
  )
})
