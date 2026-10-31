// 组件 ZIP 解包与安装路径测试。不带 ?stage=1 的 POST /api/custom-ui/import 保持 A7 S4 的行为：一律结构化拒绝、
// 不写任何文件（A8 的暂存与信任确认见 custom-ui-lifecycle.test.mjs）；importCustomUiZip 的安装路径用 v2 manifest
// 适配器直接调用，保住它的覆盖。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { strToU8, zipSync } from 'fflate'
import { normalizeNativeManifest } from '../../shared/custom-ui-manifest.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { resolveComponentFile } from '../services/custom-ui-files.mjs'
import {
  CustomUiImportError,
  importCustomUiZip,
  unpackCustomUiZip,
  CUSTOM_UI_MANIFEST_MAX_BYTES,
} from '../services/custom-ui-import.mjs'
import { customUiRoutes } from '../http/routes/custom-ui.mjs'

// v2 适配器：importCustomUiZip 期望 normalizeManifest 抛错或返回带 entry / name / version 的对象。
function normalizeV2(id, raw) {
  const { manifest } = normalizeNativeManifest(id, raw, { hostVersion: '99.0.0' })
  if (!manifest) throw new Error('manifest invalid')
  return manifest
}

const NATIVE_MANIFEST = { runtime: 'native', name: 'Native Card', version: '1.0.0' }

function legacyBundle(path = 'pisper-game-asset-workbench') {
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
    'repo-main/README.md': strToU8('Unrelated repository documentation'),
  })
}

function bundle(path = 'native-card') {
  return zipSync({
    [`repo-main/components/${path}/manifest.json`]: strToU8(JSON.stringify(NATIVE_MANIFEST)),
    [`repo-main/components/${path}/component.js`]: strToU8(
      'export default function Card({ ctx }) { return ctx.html`<p>ok</p>` }',
    ),
    'repo-main/README.md': strToU8('Unrelated repository documentation'),
  })
}

async function postImport(service, bytes, search = '') {
  const route = customUiRoutes.find(
    (entry) => entry.method === 'POST' && entry.path === '/api/custom-ui/import',
  )
  let status = 0
  let result
  await route.handler({
    services: { customUi: service },
    bodyBuffer: async () => bytes,
    url: new URL(`http://127.0.0.1/api/custom-ui/import${search}`),
    res: { headersSent: false },
    json: (code, value) => {
      status = code
      result = value
    },
  })
  return { status, result }
}

test('import route rejects legacy and native bundles with structured codes and writes nothing', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-import-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  assert.deepEqual(await postImport(service, legacyBundle()), {
    status: 409,
    result: { code: 'component_legacy_unsupported', error: 'component_legacy_unsupported' },
  })
  assert.deepEqual(await postImport(service, bundle()), {
    status: 409,
    result: { code: 'component_import_unavailable', error: 'component_import_unavailable' },
  })
  assert.deepEqual(
    await postImport(service, zipSync({ '../escape/manifest.json': strToU8('{}') })),
    {
      status: 400,
      result: { code: 'component_archive_invalid', error: 'component_archive_invalid' },
    },
  )
  assert.deepEqual(
    await postImport(service, zipSync({ 'bad/manifest.json': strToU8('{not json') })),
    {
      status: 400,
      result: { code: 'component_manifest_invalid', error: 'component_manifest_invalid' },
    },
  )
  // 请求体超限：bodyBuffer 抛错 → 413，不读也不写。
  const tooLarge = customUiRoutes.find(
    (entry) => entry.method === 'POST' && entry.path === '/api/custom-ui/import',
  )
  let status = 0
  await tooLarge.handler({
    services: { customUi: service },
    bodyBuffer: async () => {
      throw Object.assign(new Error('too large'), { statusCode: 413 })
    },
    res: { headersSent: false },
    json: (code) => {
      status = code
    },
  })
  assert.equal(status, 413)
  // 什么都没写：数据目录里没有 custom-ui/ 与暂存目录。
  assert.deepEqual(await readdir(dataDir), [])
})

test('v2 install path keeps one complete directory and does not overwrite existing data', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-component-install-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const installed = await importCustomUiZip({
    root,
    bytes: bundle(),
    normalizeManifest: normalizeV2,
  })
  assert.deepEqual(installed, { id: 'native-card', name: 'Native Card', version: '1.0.0' })
  assert.match(await readFile(join(root, 'native-card', 'component.js'), 'utf8'), /export default/)
  await assert.rejects(
    importCustomUiZip({ root, bytes: bundle(), normalizeManifest: normalizeV2 }),
    { code: 'component_already_installed', statusCode: 409 },
  )
  // 保留 id 在安装路径上同样被拒绝。
  await assert.rejects(
    importCustomUiZip({
      root,
      bytes: bundle('pisper-island'),
      normalizeManifest: normalizeV2,
      reservedIds: ['pisper-island'],
    }),
    { code: 'component_id_reserved', statusCode: 409 },
  )
  // v2 适配器拒绝旧 iframe manifest（缺 runtime:'native'）。
  await assert.rejects(
    importCustomUiZip({ root, bytes: legacyBundle('old'), normalizeManifest: normalizeV2 }),
    { code: 'component_manifest_invalid' },
  )
  assert.equal((await readdir(root)).includes('old'), false)
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
    await assert.rejects(service.rejectImport(archive), { code: /^component_(archive|manifest)_/ })
  }
  assert.equal((await readdir(dataDir)).includes('custom-ui'), false)
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
    await assert.rejects(service.rejectImport(archive), { code: /^component_archive_/ })
  assert.equal((await readdir(dataDir)).includes('custom-ui'), false)
})

test('concurrent component imports publish one complete directory and remove all staging directories', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-component-import-race-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const install = () =>
    importCustomUiZip({ root, bytes: bundle('race'), normalizeManifest: normalizeV2 })
  const results = await Promise.allSettled([install(), install()])
  assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1)
  const failure = results.find((result) => result.status === 'rejected')
  assert.equal(failure.reason.code, 'component_already_installed')
  assert.match(await readFile(join(root, 'race', 'component.js'), 'utf8'), /export default/)
  assert.ok((await readdir(root)).every((name) => !name.startsWith('.component-import-')))
})

test('Windows-style rename EPERM maps only existing targets to duplicate installs', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-component-import-windows-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'race'), { recursive: true })
  const lockedRename = async () => {
    throw Object.assign(new Error('locked'), { code: 'EPERM' })
  }
  await assert.rejects(
    importCustomUiZip({
      root,
      bytes: bundle('race'),
      normalizeManifest: normalizeV2,
      renameDirectory: lockedRename,
    }),
    { code: 'component_already_installed', statusCode: 409 },
  )
  await assert.rejects(
    importCustomUiZip({
      root,
      bytes: bundle('other'),
      normalizeManifest: normalizeV2,
      renameDirectory: lockedRename,
    }),
    { code: 'EPERM' },
  )
  assert.ok(
    (await readdir(root)).every((name) => !name.startsWith('.component-import-')),
    'failed renames must clean their staging directories',
  )
})

test('install and scan accept the same manifest byte limit and reject oversized manifests before publishing', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-manifest-limit-'))
  const service = new CustomUiService({ dataDir })
  t.after(async () => {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const base = JSON.stringify({ ...NATIVE_MANIFEST, padding: '' })
  const make = (size) =>
    strToU8(
      JSON.stringify({ ...NATIVE_MANIFEST, padding: 'x'.repeat(size - Buffer.byteLength(base)) }),
    )
  const archive = (name, size) =>
    zipSync({
      [`${name}/manifest.json`]: make(size),
      [`${name}/component.js`]: strToU8('export default function C() { return null }'),
    })
  assert.equal(make(CUSTOM_UI_MANIFEST_MAX_BYTES).byteLength, CUSTOM_UI_MANIFEST_MAX_BYTES)
  const install = (bytes) =>
    importCustomUiZip({ root: service.root, bytes, normalizeManifest: normalizeV2 })
  await install(archive('accepted', CUSTOM_UI_MANIFEST_MAX_BYTES))
  const accepted = (await service.catalog()).components.find((row) => row.id === 'accepted')
  assert.equal(accepted?.kind, 'native')
  assert.equal(accepted?.status, 'ready')
  await assert.rejects(install(archive('oversized', CUSTOM_UI_MANIFEST_MAX_BYTES + 1)), {
    code: 'component_manifest_invalid',
  })
  assert.equal((await readdir(service.root)).includes('oversized'), false)
})

test('installed nested entry and resource paths stay readable after restarting', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-component-nested-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const archive = zipSync({
    'nested/manifest.json': strToU8(JSON.stringify({ ...NATIVE_MANIFEST, entry: 'src/main.js' })),
    'nested/src/main.js': strToU8("import './lib/util.js'\nexport default function C() {}"),
    'nested/src/lib/util.js': strToU8('export const loaded = true'),
    'nested/assets/app.css': strToU8('body { color: black }'),
  })
  const root = join(dataDir, 'custom-ui')
  const installed = await importCustomUiZip({
    root,
    bytes: archive,
    normalizeManifest: normalizeV2,
  })
  assert.equal(installed.id, 'nested')
  const restarted = new CustomUiService({ dataDir })
  t.after(() => restarted.dispose())
  const component = (await restarted.catalog()).components.find((entry) => entry.id === 'nested')
  assert.equal(component.entry, 'src/main.js')
  assert.equal(component.status, 'ready')
  const asset = await resolveComponentFile(root, 'nested', 'src/lib/util.js')
  assert.equal(await readFile(asset.file, 'utf8'), 'export const loaded = true')
  assert.equal(await resolveComponentFile(root, 'nested', 'src/../../../outside.js'), null)
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
