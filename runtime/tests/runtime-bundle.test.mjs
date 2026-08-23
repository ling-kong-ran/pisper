import assert from 'node:assert/strict'
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
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

test('Runtime bundle preserves host entries and only declares external package roots', async () => {
  const runtimeDir = await mkdtemp(join(tmpdir(), 'pisper-runtime-bundle-'))
  const dependencies = Object.fromEntries(RUNTIME_EXTERNAL_PACKAGES.map((name) => [name, '1.0.0']))
  try {
    await Promise.all([
      createFile(
        runtimeDir,
        'package.json',
        `${JSON.stringify({ name: 'fixture', type: 'module', dependencies })}\n`,
      ),
      createFile(
        runtimeDir,
        'runtime/sidecar.mjs',
        "import { value } from '../shared/value.mjs'\nglobalThis.__bundleSidecar = value\n",
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
      createFile(runtimeDir, 'shared/value.mjs', "export const value = 'bundled'\n"),
    ])

    const manifest = await bundleRuntime({ runtimeDir })
    const stagedPackage = JSON.parse(await readFile(join(runtimeDir, 'package.json'), 'utf8'))

    assert.equal(manifest.schema, RUNTIME_BUNDLE_SCHEMA)
    assert.deepEqual(manifest.entries, ['runtime/sidecar.mjs', 'runtime/mobile-embedded.mjs'])
    assert.deepEqual(Object.keys(stagedPackage.dependencies), [...RUNTIME_EXTERNAL_PACKAGES])
    assert.equal(await exists(join(runtimeDir, 'shared')), false)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'sidecar.mjs')), true)
    assert.equal(await exists(join(runtimeDir, 'runtime', 'mobile-embedded.mjs')), true)
    assert.equal(await exists(join(runtimeDir, 'THIRD_PARTY_LICENSES.txt')), true)
    assert.equal(
      await exists(join(runtimeDir, 'runtime', 'plugins', 'local-plugin-worker.mjs')),
      true,
    )
    assert.ok(manifest.inputFileCount >= 3)
    assert.ok(manifest.outputFileCount >= 3)

    await import(pathToFileURL(join(runtimeDir, 'runtime', 'sidecar.mjs')).href)
    await import(pathToFileURL(join(runtimeDir, 'runtime', 'mobile-embedded.mjs')).href)
    assert.equal(globalThis.__bundleSidecar, 'bundled')
    assert.equal(globalThis.__bundleMobile, 'bundled')
  } finally {
    delete globalThis.__bundleSidecar
    delete globalThis.__bundleMobile
    await rm(runtimeDir, { recursive: true, force: true })
  }
})

test('Runtime bundle rejects a missing external production dependency', async () => {
  const runtimeDir = await mkdtemp(join(tmpdir(), 'pisper-runtime-bundle-missing-'))
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
