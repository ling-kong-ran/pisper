import assert from 'node:assert/strict'
import { writeFileSync } from 'node:fs'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'
import test from 'node:test'
import tar from 'tar-stream'
import { download, sha256 } from '../upstream-download.mjs'
import { PI_CATALOG_PATH, updatePiModelCatalog } from '../update-pi-model-catalog.mjs'
import {
  crateFiles,
  latestStable,
  mergePatchedFiles,
  PI_RUST_PIN,
  updatePiRust,
} from '../update-pi-rust.mjs'

const model = {
  id: 'new-model',
  name: 'New model',
  type: 'chat',
  provider: 'fixture',
  api: 'anthropic-messages',
  baseUrl: 'https://fixture.invalid',
  input: ['text'],
  reasoning: true,
  contextWindow: 1_000_000,
  maxTokens: 128000,
  cost: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0 },
  compat: { forceAdaptiveThinking: true, supportsTemperature: false },
}
async function temporary(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-upstream-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  return root
}
function catalogFetch(data = { fixture: [model] }, digest) {
  const bytes = Buffer.from(JSON.stringify(data))
  return () =>
    new Response(bytes, {
      headers: { 'x-pi-model-catalog-revision': `sha256-${digest || sha256(bytes)}` },
    })
}

test('catalog update preserves full API metadata, is deterministic and accepts typed duplicate ids', async (t) => {
  const root = await temporary(t)
  const fetchImpl = catalogFetch({ fixture: [model, { ...model, type: 'image' }] })
  assert.equal((await updatePiModelCatalog({ root, fetchImpl })).changed, true)
  const saved = await readFile(join(root, PI_CATALOG_PATH), 'utf8')
  assert.equal(JSON.parse(saved).models.fixture[0].compat.forceAdaptiveThinking, true)
  assert.equal((await updatePiModelCatalog({ root, fetchImpl })).changed, false)
  assert.equal(await readFile(join(root, PI_CATALOG_PATH), 'utf8'), saved)
})

test('invalid checksum, schema, duplicate chat and network errors preserve the old snapshot', async (t) => {
  const root = await temporary(t)
  await updatePiModelCatalog({ root, fetchImpl: catalogFetch() })
  const saved = await readFile(join(root, PI_CATALOG_PATH), 'utf8')
  for (const fetchImpl of [
    catalogFetch(undefined, '0'.repeat(64)),
    catalogFetch({}),
    catalogFetch({ fixture: [model, model] }),
    catalogFetch({ fixture: [{ ...model, contextWindow: -1 }] }),
    () => {
      throw new Error('offline')
    },
  ]) {
    await assert.rejects(updatePiModelCatalog({ root, fetchImpl }))
    assert.equal(await readFile(join(root, PI_CATALOG_PATH), 'utf8'), saved)
  }
})

test('downloads reject HTTP errors and oversized streams', async () => {
  await assert.rejects(
    download('https://fixture.invalid', {
      fetchImpl: () => new Response('private error', { status: 500 }),
    }),
    /HTTP 500/,
  )
  await assert.rejects(
    download('https://fixture.invalid', {
      maxBytes: 2,
      fetchImpl: () => new Response('large'),
    }),
    /size limit/,
  )
})

test('Rust version selection excludes prereleases and yanked versions and refuses downgrades', () => {
  const entry = (vers, yanked = false) =>
    JSON.stringify({ name: 'pi-rs', vers, yanked, cksum: 'a'.repeat(64) })
  assert.equal(
    latestStable(
      [entry('0.2.2'), entry('0.3.0'), entry('0.4.0', true), entry('1.0.0-beta')].join('\n'),
      '0.2.2',
    ).vers,
    '0.3.0',
  )
  assert.throws(() => latestStable(entry('0.1.0'), '0.2.2'), /older/)
})

async function archive(version, body, extra = []) {
  const pack = tar.pack(),
    chunks = []
  const completed = new Promise((resolveDone, reject) => {
    pack.on('data', (bytes) => chunks.push(bytes))
    pack.on('end', resolveDone)
    pack.on('error', reject)
  })
  for (const [name, content] of Object.entries({
    'Cargo.toml': `[package]\nname = "pi-rs"\nversion = "${version}"\nlicense = "MIT"\n`,
    LICENSE: 'MIT fixture',
    'src/lib.rs': body,
  }))
    pack.entry({ name: `pi-rs-${version}/${name}`, type: 'file' }, content)
  for (const header of extra) pack.entry(header, '')
  pack.finalize()
  await completed
  return gzipSync(Buffer.concat(chunks))
}

test('crate extraction rejects path traversal and symlinks', async () => {
  for (const header of [
    { name: 'pi-rs-0.2.2/../escape', type: 'file' },
    { name: 'pi-rs-0.2.2/link', type: 'symlink', linkname: '/tmp/escape' },
  ])
    await assert.rejects(crateFiles(await archive('0.2.2', 'fixture', [header]), '0.2.2'))
})

test('three-way merge preserves Pisper patches and new upstream changes; conflicts fail closed', async (t) => {
  const root = await temporary(t)
  const before =
    ['base', ...Array.from({ length: 12 }, (_, i) => `line ${i}`), 'tail'].join('\n') + '\n'
  const base = new Map([['lib.rs', Buffer.from(before)]])
  const local = new Map([
    ['lib.rs', Buffer.from(before.replace('tail', 'Pisper patch'))],
    ['regression.rs', Buffer.from('local test')],
  ])
  const upstream = new Map([['lib.rs', Buffer.from(before.replace('base', 'upstream'))]])
  const merged = await mergePatchedFiles(base, local, upstream, root)
  assert.match(merged.get('lib.rs').toString(), /upstream/)
  assert.match(merged.get('lib.rs').toString(), /Pisper patch/)
  assert(merged.has('regression.rs'))
  upstream.set('lib.rs', Buffer.from(before.replace('tail', 'conflicting update')))
  await assert.rejects(mergePatchedFiles(base, local, upstream, root), /patch conflict/)
})

test('Rust updater updates both lockfiles and rolls back the entire source on Cargo failure', async (t) => {
  for (const failure of ['none', 'cargo', 'checksum']) {
    const fail = failure !== 'none'
    const root = await temporary(t),
      before = 'base\n'
    const oldArchive = await archive('0.2.2', before),
      newArchive = await archive('0.3.0', before)
    const oldFiles = await crateFiles(oldArchive, '0.2.2')
    oldFiles.set('src/lib.rs', Buffer.from('Pisper patch\n'))
    for (const [name, bytes] of oldFiles) {
      const file = join(root, 'runtime-rs/vendor/pi-rs', name)
      await mkdir(join(file, '..'), { recursive: true })
      await writeFile(file, bytes)
    }
    await mkdir(join(root, 'src-tauri'))
    await writeFile(
      join(root, PI_RUST_PIN),
      JSON.stringify({
        schemaVersion: 1,
        crate: 'pi-rs',
        version: '0.2.2',
        checksum: sha256(oldArchive),
      }),
    )
    await writeFile(join(root, 'runtime-rs/Cargo.toml'), 'pi-rs = "0.2.2"\n')
    for (const component of ['runtime-rs', 'src-tauri'])
      await writeFile(join(root, component, 'Cargo.lock'), 'old-lock')
    const calls = []
    const fetchImpl = (url) =>
      new Response(
        url.includes('index.crates.io')
          ? JSON.stringify({
              name: 'pi-rs',
              vers: '0.3.0',
              yanked: false,
              cksum: sha256(newArchive),
            })
          : url.includes('0.3.0')
            ? failure === 'checksum'
              ? Buffer.from('tampered archive')
              : newArchive
            : oldArchive,
      )
    const runCargo = (args) => {
      calls.push(args)
      const lock = args[args.indexOf('--manifest-path') + 1].replace('Cargo.toml', 'Cargo.lock')
      writeFileSync(join(root, lock), 'new-lock')
      if (failure === 'cargo' && calls.length === 2) throw new Error('cargo failed')
    }
    if (fail)
      await assert.rejects(
        updatePiRust({ root, fetchImpl, runCargo }),
        /cargo failed|checksum mismatch/,
      )
    else
      assert.deepEqual(await updatePiRust({ root, fetchImpl, runCargo }), {
        changed: true,
        version: '0.3.0',
      })
    assert.equal(
      JSON.parse(await readFile(join(root, PI_RUST_PIN))).version,
      fail ? '0.2.2' : '0.3.0',
    )
    assert.equal(
      await readFile(join(root, 'runtime-rs/vendor/pi-rs/src/lib.rs'), 'utf8'),
      'Pisper patch\n',
    )
    assert.equal(
      await readFile(join(root, 'runtime-rs/Cargo.toml'), 'utf8'),
      fail ? 'pi-rs = "0.2.2"\n' : 'pi-rs = "=0.3.0"\n',
    )
    assert.equal(calls.length, failure === 'checksum' ? 0 : 2)
    for (const component of ['runtime-rs', 'src-tauri'])
      assert.equal(
        await readFile(join(root, component, 'Cargo.lock'), 'utf8'),
        fail ? 'old-lock' : 'new-lock',
      )
  }
})
