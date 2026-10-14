import assert from 'node:assert/strict'
import { createHash, randomBytes } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createGunzip } from 'node:zlib'
import { extract as extractTar, list as listTar } from 'tar'
import { createMobileRuntimeArchive } from '../../scripts/mobile-runtime-archive.mjs'

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-archive-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const runtimeDir = join(root, 'runtime')
  await mkdir(join(runtimeDir, 'runtime'), { recursive: true })
  await mkdir(join(runtimeDir, 'dist'), { recursive: true })
  await writeFile(join(runtimeDir, 'runtime/mobile-embedded.mjs'), 'export const ready = true\n')
  await writeFile(join(runtimeDir, 'dist/index.html'), '<!doctype html><div id="root"></div>\n')
  return {
    runtimeDir,
    output: join(root, 'runtime.tgz'),
    appVersion: '1.2.3',
    runtimeProfile: 'mobile-embedded',
  }
}

async function firstManifest(output) {
  const bytes = await readFile(output)
  // 与原生探测相同，只向解压器提供最多 16 KiB，不要求读取完整 gzip 尾部。
  const gunzip = createGunzip({ finishFlush: 2 })
  gunzip.end(bytes.subarray(0, 16 * 1024))
  const chunks = []
  for await (const chunk of gunzip) {
    chunks.push(chunk)
    if (chunks.reduce((sum, value) => sum + value.length, 0) >= 9 * 1024) break
  }
  const unpacked = Buffer.concat(chunks)
  assert.equal(unpacked.subarray(0, 100).toString().split('\0')[0], './embedded-runtime.json')
  const size = Number.parseInt(
    unpacked.subarray(124, 136).toString().replaceAll('\0', '').trim(),
    8,
  )
  assert.ok(size <= 8 * 1024)
  return JSON.parse(unpacked.subarray(512, 512 + size).toString())
}

test('首条小清单可在 16 KiB 内读取，并覆盖两个实际入口摘要', async (t) => {
  const options = await fixture(t)
  await writeFile(join(options.runtimeDir, 'large.bin'), randomBytes(256 * 1024))
  const manifest = await createMobileRuntimeArchive(options)
  assert.deepEqual(await firstManifest(options.output), manifest)
  assert.equal(manifest.schemaVersion, 1)
  assert.equal(manifest.appVersion, options.appVersion)
  assert.equal(manifest.runtimeProfile, options.runtimeProfile)
  assert.match(manifest.buildSha256, /^[a-f0-9]{64}$/)
  for (const [field, path] of [
    ['entrySha256', 'runtime/mobile-embedded.mjs'],
    ['frontendSha256', 'dist/index.html'],
  ]) {
    assert.equal(
      manifest[field],
      createHash('sha256')
        .update(await readFile(join(options.runtimeDir, path)))
        .digest('hex'),
    )
  }
})

test('重建排除旧清单且不受 mtime 影响，归档没有重复条目', async (t) => {
  const options = await fixture(t)
  const first = await createMobileRuntimeArchive(options)
  const bytes = await readFile(options.output)
  await utimes(join(options.runtimeDir, 'runtime/mobile-embedded.mjs'), 123456, 123456)
  await writeFile(join(options.runtimeDir, 'embedded-runtime.json'), '{"stale":true}')
  assert.deepEqual(await createMobileRuntimeArchive(options), first)
  assert.deepEqual(await readFile(options.output), bytes)
  const paths = []
  await listTar({ file: options.output, onReadEntry: (entry) => paths.push(entry.path) })
  assert.equal(paths[0], './embedded-runtime.json')
  assert.equal(paths.filter((path) => path === './embedded-runtime.json').length, 1)
  assert.equal(paths.length, new Set(paths).size)
})

test('同 App 版本的闭包、前端、版本或 profile 变化都会改变 build 指纹', async (t) => {
  const options = await fixture(t)
  const initial = await createMobileRuntimeArchive(options)
  await writeFile(join(options.runtimeDir, 'dependency.mjs'), 'export const value = 1\n')
  const dependency = await createMobileRuntimeArchive(options)
  assert.notEqual(dependency.buildSha256, initial.buildSha256)
  assert.equal(dependency.entrySha256, initial.entrySha256)
  await writeFile(join(options.runtimeDir, 'dist/index.html'), '<!doctype html>updated')
  const frontend = await createMobileRuntimeArchive(options)
  assert.notEqual(frontend.buildSha256, dependency.buildSha256)
  assert.notEqual(frontend.frontendSha256, dependency.frontendSha256)
  const store = await createMobileRuntimeArchive({ ...options, runtimeProfile: 'mobile-store' })
  assert.notEqual(store.buildSha256, frontend.buildSha256)
  const version = await createMobileRuntimeArchive({ ...options, appVersion: '1.2.4' })
  assert.notEqual(version.buildSha256, frontend.buildSha256)
})

for (const runtimeProfile of ['mobile-embedded', 'mobile-store']) {
  test(`${runtimeProfile} 同版本实际归档重建区分不同 build 并可还原原指纹`, async (t) => {
    const options = { ...(await fixture(t)), runtimeProfile }
    const entryPath = join(options.runtimeDir, 'runtime/mobile-embedded.mjs')
    const originalEntry = await readFile(entryPath)
    const original = await createMobileRuntimeArchive(options)
    const originalArchive = await readFile(options.output)
    const updatedEntry = 'export const ready = "updated build"\n'
    await writeFile(entryPath, updatedEntry)
    const updated = await createMobileRuntimeArchive(options)
    assert.equal(updated.appVersion, original.appVersion)
    assert.equal(updated.runtimeProfile, original.runtimeProfile)
    assert.notEqual(updated.buildSha256, original.buildSha256)
    assert.notEqual(updated.entrySha256, original.entrySha256)
    assert.equal(updated.frontendSha256, original.frontendSha256)
    assert.notDeepEqual(await readFile(options.output), originalArchive)
    assert.deepEqual(await firstManifest(options.output), updated)

    const extracted = `${options.runtimeDir}-extracted`
    await mkdir(extracted)
    await extractTar({ file: options.output, cwd: extracted })
    assert.deepEqual(
      JSON.parse(await readFile(join(extracted, 'embedded-runtime.json'), 'utf8')),
      updated,
    )
    assert.equal(await readFile(join(extracted, updated.entry), 'utf8'), updatedEntry)

    await writeFile(entryPath, originalEntry)
    assert.deepEqual(await createMobileRuntimeArchive(options), original)
    assert.deepEqual(await readFile(options.output), originalArchive)
  })
}

test('缺失、空入口及越界清单在发布归档前失败', async (t) => {
  const options = await fixture(t)
  await assert.rejects(
    createMobileRuntimeArchive({ ...options, runtimeProfile: 'desktop' }),
    /Invalid/,
  )
  await assert.rejects(createMobileRuntimeArchive({ ...options, appVersion: '' }), /Invalid/)
  await assert.rejects(
    createMobileRuntimeArchive({ ...options, appVersion: '1'.repeat(8192) }),
    /probe limit/,
  )
  await writeFile(join(options.runtimeDir, 'runtime/mobile-embedded.mjs'), '')
  await assert.rejects(createMobileRuntimeArchive(options), /Empty/)
  await rm(join(options.runtimeDir, 'runtime/mobile-embedded.mjs'))
  await assert.rejects(createMobileRuntimeArchive(options), /ENOENT/)
  await assert.rejects(readFile(options.output), /ENOENT/)
})
