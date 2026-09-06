import assert from 'node:assert/strict'
import childProcess from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises'
import http from 'node:http'
import https from 'node:https'
import { syncBuiltinESMExports } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { stageAndroidSpeechResources } from '../../scripts/stage-android-speech-model.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const catalogPath = 'speech-model-catalog.json'
const sourceCatalogPath = catalogPath
const noticesPath = 'speech-resource-notices.json'
const bpePath = 'speech-resources/xasr-bpe.vocab'

async function fixture(t) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'pisper-speech-resources-')))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const sourceDir = join(dir, 'shared')
  const targetDir = join(dir, 'android', 'app', 'src', 'main', 'assets')
  await mkdir(join(sourceDir, 'speech-resources'), { recursive: true })
  await mkdir(join(targetDir, 'speech-model'), { recursive: true })
  for (const path of [sourceCatalogPath, noticesPath, bpePath]) {
    await copyFile(join(root, 'shared', path), join(sourceDir, path))
  }
  await writeFile(join(targetDir, 'pisper-embedded-runtime.tgz'), 'existing-runtime')
  await writeFile(join(targetDir, catalogPath), 'existing-catalog')
  await writeFile(join(targetDir, noticesPath), 'existing-notices')
  await writeFile(join(targetDir, 'speech-model', 'encoder.int8.onnx'), 'old-weights')
  return { dir, sourceDir, targetDir }
}

async function assertUnchanged(targetDir) {
  assert.equal(
    await readFile(join(targetDir, 'pisper-embedded-runtime.tgz'), 'utf8'),
    'existing-runtime',
  )
  assert.equal(await readFile(join(targetDir, catalogPath), 'utf8'), 'existing-catalog')
  assert.equal(await readFile(join(targetDir, noticesPath), 'utf8'), 'existing-notices')
  assert.equal(
    await readFile(join(targetDir, 'speech-model', 'encoder.int8.onnx'), 'utf8'),
    'old-weights',
  )
}

test('stages only trusted small speech resources offline and preserves embedded runtime', async (t) => {
  const input = await fixture(t)
  await writeFile(join(input.sourceDir, 'unrelated.onnx'), 'not-an-asset')
  await writeFile(join(input.sourceDir, 'speech-resources', 'unexpected.onnx'), 'not-an-asset')
  await mkdir(join(input.targetDir, 'speech-resources'))
  await writeFile(join(input.targetDir, 'speech-resources', 'obsolete.onnx'), 'stale-weight')
  const calls = []
  const forbidden = (name) => () => {
    calls.push(name)
    throw new Error(`Unexpected network or child process: ${name}`)
  }
  t.mock.method(globalThis, 'fetch', forbidden('fetch'))
  for (const api of [http, https]) {
    for (const name of ['request', 'get']) t.mock.method(api, name, forbidden(name))
  }
  for (const name of [
    'spawn',
    'spawnSync',
    'exec',
    'execSync',
    'execFile',
    'execFileSync',
    'fork',
  ]) {
    t.mock.method(childProcess, name, forbidden(name))
  }
  syncBuiltinESMExports()
  try {
    assert.equal(await stageAndroidSpeechResources(input), input.targetDir)
    assert.equal(await stageAndroidSpeechResources(input), input.targetDir)
  } finally {
    t.mock.restoreAll()
    syncBuiltinESMExports()
  }
  assert.deepEqual(calls, [])
  assert.deepEqual(
    (await readdir(input.targetDir)).sort(),
    ['pisper-embedded-runtime.tgz', catalogPath, noticesPath, 'speech-resources'].sort(),
  )
  assert.deepEqual(await readdir(join(input.targetDir, 'speech-resources')), ['xasr-bpe.vocab'])
  assert.equal(
    await readFile(join(input.targetDir, 'pisper-embedded-runtime.tgz'), 'utf8'),
    'existing-runtime',
  )
  for (const [source, target] of [
    [sourceCatalogPath, catalogPath],
    [noticesPath, noticesPath],
    [bpePath, bpePath],
  ]) {
    assert.deepEqual(
      await readFile(join(input.targetDir, target)),
      await readFile(join(input.sourceDir, source)),
    )
  }
  const bpe = await readFile(join(input.targetDir, bpePath))
  assert.equal(bpe.length, 61562)
  assert.equal(
    createHash('sha256').update(bpe).digest('hex'),
    '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5',
  )
  await assert.rejects(lstat(join(input.targetDir, 'speech-model')), { code: 'ENOENT' })
})

for (const [name, mutate] of [
  [
    'unsupported version',
    (catalog) => {
      catalog.version = 2
    },
  ],
  [
    'empty models',
    (catalog) => {
      catalog.models = []
    },
  ],
  [
    'duplicate model id',
    (catalog) => {
      catalog.models.push(catalog.models[0])
    },
  ],
  [
    'unknown default',
    (catalog) => {
      catalog.defaults.asr = 'missing'
    },
  ],
  [
    'unsafe model file',
    (catalog) => {
      catalog.models[0].files[0].path = '../escape.onnx'
    },
  ],
  [
    'invalid file hash',
    (catalog) => {
      catalog.models[0].files[0].sha256 = 'invalid'
    },
  ],
  [
    'resource traversal',
    (catalog) => {
      catalog.models[0].config.bpeVocabResource = '../outside'
    },
  ],
  [
    'unlisted resource',
    (catalog) => {
      catalog.models[0].config.bpeVocabResource = 'speech-resources/weights.onnx'
    },
  ],
]) {
  test(`rejects ${name} before modifying existing assets`, async (t) => {
    const input = await fixture(t)
    const catalog = JSON.parse(await readFile(join(input.sourceDir, sourceCatalogPath), 'utf8'))
    mutate(catalog)
    await writeFile(join(input.sourceDir, sourceCatalogPath), JSON.stringify(catalog))
    await assert.rejects(
      stageAndroidSpeechResources(input),
      /catalog structure or resource allowlist/,
    )
    await assertUnchanged(input.targetDir)
  })
}

for (const corruption of [
  'invalid-json',
  'oversized-catalog',
  'missing-bpe',
  'corrupt-bpe',
  'missing-catalog',
  'missing-notices',
  'invalid-notices',
  'oversized-notices',
  'wrong-notice-revision',
]) {
  test(`rejects ${corruption} without deleting old assets`, async (t) => {
    const input = await fixture(t)
    if (corruption === 'invalid-json')
      await writeFile(join(input.sourceDir, sourceCatalogPath), '{')
    if (corruption === 'oversized-catalog') {
      await writeFile(join(input.sourceDir, sourceCatalogPath), Buffer.alloc(4 * 1024 * 1024 + 1))
    }
    if (corruption === 'missing-catalog') await rm(join(input.sourceDir, sourceCatalogPath))
    if (corruption === 'missing-notices') await rm(join(input.sourceDir, noticesPath))
    if (corruption === 'invalid-notices') await writeFile(join(input.sourceDir, noticesPath), '{')
    if (corruption === 'oversized-notices') {
      await writeFile(join(input.sourceDir, noticesPath), Buffer.alloc(128 * 1024 + 1))
    }
    if (corruption === 'wrong-notice-revision') {
      const notices = JSON.parse(await readFile(join(input.sourceDir, noticesPath), 'utf8'))
      notices.models.forEach((model) => {
        model.revision = 'unmatched'
      })
      await writeFile(join(input.sourceDir, noticesPath), JSON.stringify(notices))
    }
    if (corruption === 'missing-bpe') await rm(join(input.sourceDir, bpePath))
    if (corruption === 'corrupt-bpe') {
      const bpe = await readFile(join(input.sourceDir, bpePath))
      bpe[0] ^= 1
      await writeFile(join(input.sourceDir, bpePath), bpe)
    }
    await assert.rejects(stageAndroidSpeechResources(input))
    await assertUnchanged(input.targetDir)
  })
}

test('rejects old target semantics, overlapping source, and empty arguments', async (t) => {
  const input = await fixture(t)
  for (const targetDir of [
    join(input.targetDir, 'speech-model'),
    join(input.sourceDir, 'assets'),
    '',
  ]) {
    await assert.rejects(stageAndroidSpeechResources({ sourceDir: input.sourceDir, targetDir }))
  }
  await assert.rejects(stageAndroidSpeechResources({ sourceDir: '', targetDir: input.targetDir }))
  await assertUnchanged(input.targetDir)
})

test('rejects symlinked asset directories without touching their destinations', async (t) => {
  const input = await fixture(t)
  const external = join(input.dir, 'external')
  await mkdir(external)
  await writeFile(join(external, 'sentinel'), 'keep')
  await symlink(
    external,
    join(input.targetDir, 'speech-resources'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(stageAndroidSpeechResources(input), /rejects symlinks/)
  assert.equal(await readFile(join(external, 'sentinel'), 'utf8'), 'keep')
  await assertUnchanged(input.targetDir)
})
