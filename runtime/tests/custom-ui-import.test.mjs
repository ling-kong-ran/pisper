import assert from 'node:assert/strict'
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { strToU8, zipSync } from 'fflate'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import {
  CustomUiImportError,
  unpackCustomUiZip,
  CUSTOM_UI_MANIFEST_MAX_BYTES,
} from '../services/custom-ui-import.mjs'
import { customUiRoutes } from '../http/routes/custom-ui.mjs'

function bundle(path = 'pisper-game-asset-workbench') {
  return zipSync({
    [`repo-main/components/${path}/manifest.json`]: strToU8(
      JSON.stringify({
        name: 'Game Asset Workbench',
        version: '1.0.0',
        entry: 'index.html',
        permissions: ['game-assets.read', 'game-assets.write', 'game-assets.run'],
      }),
    ),
    [`repo-main/components/${path}/index.html`]: strToU8('<!doctype html><title>Workbench</title>'),
    [`repo-main/components/${path}/frame-editor.js`]: strToU8('window.frameEditor = true'),
    'repo-main/README.md': strToU8('Unrelated repository documentation'),
  })
}

test('imported component becomes a normal user component without overwriting existing data', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-import-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const route = customUiRoutes.find(
    (entry) => entry.method === 'POST' && entry.path === '/api/custom-ui/import',
  )
  let status = 0
  let result
  await route.handler({
    services: { customUi: service },
    bodyBuffer: async () => bundle(),
    json: (code, value) => {
      status = code
      result = value
    },
  })
  assert.equal(status, 201)
  assert.deepEqual(result, {
    id: 'pisper-game-asset-workbench',
    name: 'Game Asset Workbench',
    version: '1.0.0',
  })
  const catalog = await service.listComponents()
  const imported = catalog.components.find((item) => item.id === result.id)
  assert.equal(imported.builtIn, undefined)
  assert.deepEqual(imported.permissions, [
    'game-assets.read',
    'game-assets.write',
    'game-assets.run',
  ])
  assert.match(
    await readFile(join(dataDir, 'custom-ui', result.id, 'index.html'), 'utf8'),
    /Workbench/,
  )
  await assert.rejects(service.importBundle(bundle()), {
    code: 'component_already_installed',
    statusCode: 409,
  })
})

test('component import rejects traversal, duplicate manifests and oversized expanded files', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-import-invalid-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  for (const archive of [
    zipSync({ '../escape/manifest.json': strToU8('{}') }),
    zipSync({
      'one/manifest.json': strToU8('{}'),
      'two/manifest.json': strToU8('{}'),
    }),
    zipSync({ 'large/asset.bin': new Uint8Array(8 * 1024 * 1024 + 1) }),
  ]) {
    await assert.rejects(service.importBundle(archive), CustomUiImportError)
  }
  assert.equal(
    (await service.listComponents()).components.some((item) => item.id === 'escape'),
    false,
  )
})

function mutateEntry(bytes, path, change) {
  const output = Uint8Array.from(bytes)
  const view = new DataView(output.buffer)
  let offset = view.getUint32(output.length - 6, true)
  const count = view.getUint16(output.length - 12, true)
  for (let index = 0; index < count; index++) {
    const nameLength = view.getUint16(offset + 28, true)
    const name = new TextDecoder().decode(output.subarray(offset + 46, offset + 46 + nameLength))
    if (name === path) {
      change(view, offset, view.getUint32(offset + 42, true))
      return output
    }
    offset +=
      46 + nameLength + view.getUint16(offset + 30, true) + view.getUint16(offset + 32, true)
  }
  throw new Error('Fixture entry not found')
}

test('component ZIP accepts ordinary repository directory records and validates all selected file bytes', () => {
  const archive = zipSync({
    'repo/': new Uint8Array(),
    'repo/components/': new Uint8Array(),
    'repo/components/example/': new Uint8Array(),
    'repo/components/example/manifest.json': strToU8('{"name":"Example"}'),
    'repo/components/example/index.html': strToU8('<h1>示例</h1>'),
    'repo/README.md': strToU8('Excluded'),
  })
  const result = unpackCustomUiZip(archive)
  assert.equal(result.id, 'example')
  assert.equal(result.assets.get('index.html').toString('utf8'), '<h1>示例</h1>')
  assert.deepEqual([...result.assets.keys()], ['manifest.json', 'index.html'])
  for (const mutate of [
    (view, central, local) => {
      view.setUint32(central + 16, 1, true)
      view.setUint32(local + 14, 1, true)
    },
    (view, central) => view.setUint32(central + 38, 0xa1ff << 16, true),
    (view, central, local) => {
      view.setUint16(central + 8, 1, true)
      view.setUint16(local + 6, 1, true)
    },
  ])
    assert.throws(
      () => unpackCustomUiZip(mutateEntry(archive, 'repo/components/example/index.html', mutate)),
      CustomUiImportError,
    )
})

test('component import rejects forged decompression sizes and case collisions before installing files', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-import-bounds-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const compressed = zipSync({
    'example/manifest.json': strToU8('{"name":"Example","entry":"index.html"}'),
    'example/index.html': new Uint8Array(9 * 1024 * 1024),
  })
  const forged = mutateEntry(compressed, 'example/index.html', (view, central, local) => {
    view.setUint32(central + 24, 1, true)
    view.setUint32(local + 22, 1, true)
  })
  for (const archive of [
    forged,
    zipSync({
      'example/manifest.json': strToU8('{"name":"Example"}'),
      'example/index.html': strToU8('first'),
      'example/INDEX.html': strToU8('second'),
    }),
  ])
    await assert.rejects(service.importBundle(archive), CustomUiImportError)
  assert.equal(
    (await service.listComponents()).components.some((component) => component.id === 'example'),
    false,
  )
})

test('concurrent component imports publish one complete directory and remove all staging directories', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-import-race-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const results = await Promise.allSettled([
    service.importBundle(bundle('race')),
    service.importBundle(bundle('race')),
  ])
  assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1)
  const failure = results.find((result) => result.status === 'rejected')
  assert.equal(failure.reason.code, 'component_already_installed')
  assert.match(
    await readFile(join(dataDir, 'custom-ui', 'race', 'index.html'), 'utf8'),
    /Workbench/,
  )
  assert.ok(
    (await readdir(join(dataDir, 'custom-ui'))).every(
      (name) => !name.startsWith('.component-import-'),
    ),
  )
})

test('import and scan accept the same manifest byte limit and reject oversized manifests before publishing', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-manifest-limit-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const base = JSON.stringify({ name: 'Manifest boundary', entry: 'index.html', padding: '' })
  const make = (size) =>
    strToU8(
      JSON.stringify({
        name: 'Manifest boundary',
        entry: 'index.html',
        padding: 'x'.repeat(size - Buffer.byteLength(base)),
      }),
    )
  const archive = (name, size) =>
    zipSync({
      [`${name}/manifest.json`]: make(size),
      [`${name}/index.html`]: strToU8('<title>Boundary</title>'),
    })
  assert.equal(make(CUSTOM_UI_MANIFEST_MAX_BYTES).byteLength, CUSTOM_UI_MANIFEST_MAX_BYTES)
  await service.importBundle(archive('accepted', CUSTOM_UI_MANIFEST_MAX_BYTES))
  assert.equal(
    (await service.listComponents()).components.some((component) => component.id === 'accepted'),
    true,
  )
  const restarted = new CustomUiService({ dataDir })
  t.after(() => restarted.dispose())
  assert.equal(
    (await restarted.listComponents()).components.some((component) => component.id === 'accepted'),
    true,
  )
  await assert.rejects(
    service.importBundle(archive('oversized', CUSTOM_UI_MANIFEST_MAX_BYTES + 1)),
    { code: 'component_manifest_invalid' },
  )
  assert.equal((await readdir(join(dataDir, 'custom-ui'))).includes('oversized'), false)
})

test('component import retains safe nested entry and resource paths which remain readable after restarting', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-nested-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const archive = zipSync({
    'nested/manifest.json': strToU8(
      JSON.stringify({ name: 'Nested assets', entry: 'pages/index.html' }),
    ),
    'nested/pages/index.html': strToU8('<script src="../assets/js/app.js"></script>'),
    'nested/assets/js/app.js': strToU8('window.loaded = true'),
    'nested/assets/css/app.css': strToU8('body { color: black }'),
  })
  const imported = await service.importBundle(archive)
  assert.equal(imported.id, 'nested')
  service.dispose()
  const restarted = new CustomUiService({ dataDir })
  t.after(() => restarted.dispose())
  const component = (await restarted.listComponents()).components.find(
    (entry) => entry.id === 'nested',
  )
  assert.equal(component.entry, 'pages/index.html')
  const view = await restarted.createView('nested')
  assert.match(view.entryUrl, /\/assets\/pages\/index\.html$/)
  const asset = await restarted.resolveAssetPath('nested', 'assets/js/app.js')
  assert.equal(await readFile(asset.file, 'utf8'), 'window.loaded = true')
  assert.equal(await restarted.resolveAssetPath('nested', 'assets/../../../outside.js'), null)
})

test('nested component import rejects hidden paths, traversals, file-directory and case-folded directory collisions', () => {
  const files = {
    'nested/manifest.json': strToU8('{"name":"Nested"}'),
    'nested/index.html': strToU8('<title>Nested</title>'),
  }
  for (const additions of [
    { 'nested/assets/.hidden.js': strToU8('hidden') },
    { 'nested/.private/app.js': strToU8('hidden') },
    { 'nested/assets/../outside.js': strToU8('traversal') },
    { 'nested/assets': strToU8('file'), 'nested/assets/app.js': strToU8('child') },
    { 'nested/Assets/app.js': strToU8('upper'), 'nested/assets/style.css': strToU8('lower') },
    { 'nested/assets/app.js': strToU8('one'), 'nested/assets/APP.js': strToU8('two') },
  ])
    assert.throws(() => unpackCustomUiZip(zipSync({ ...files, ...additions })), {
      code: 'component_archive_invalid',
    })
})
