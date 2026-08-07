import assert from 'node:assert/strict'
import { access, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import {
  SEA_RUNTIME_BUDGET_BYTES,
  assertSizeManifest,
  collectNativeState,
  createSizeManifest,
  criticalRuntimeEntries,
  detectLinuxLibc,
  finalizeSizeManifest,
  inspectCriticalFiles,
  pruneRuntime,
  selectClipboardPackage,
  selectPiTuiNativeFiles,
} from '../../scripts/sea-runtime.mjs'

async function createFile(root, relativePath, contents = 'runtime') {
  const path = join(root, ...relativePath.split('/'))
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, contents)
  return path
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

test('SEA runtime native selection is conservative and platform-specific', () => {
  assert.equal(
    selectClipboardPackage({ platform: 'win32', arch: 'x64' }),
    'clipboard-win32-x64-msvc',
  )
  assert.equal(
    selectClipboardPackage({ platform: 'darwin', arch: 'arm64' }),
    'clipboard-darwin-universal',
  )
  assert.equal(
    selectClipboardPackage({ platform: 'linux', arch: 'x64', libc: 'gnu' }),
    'clipboard-linux-x64-gnu',
  )
  assert.equal(
    selectClipboardPackage({ platform: 'linux', arch: 'arm64', libc: 'musl' }),
    'clipboard-linux-arm64-musl',
  )
  assert.equal(
    selectClipboardPackage({ platform: 'linux', arch: 'riscv64', libc: 'gnu' }),
    'clipboard-linux-riscv64-gnu',
  )
  assert.equal(selectClipboardPackage({ platform: 'linux', arch: 'x64' }), null)
  assert.equal(selectClipboardPackage({ platform: 'freebsd', arch: 'x64' }), null)

  assert.deepEqual(selectPiTuiNativeFiles({ platform: 'win32', arch: 'x64' }), [
    'native/win32/prebuilds/win32-x64/win32-console-mode.node',
  ])
  assert.deepEqual(selectPiTuiNativeFiles({ platform: 'darwin', arch: 'arm64' }), [
    'native/darwin/prebuilds/darwin-arm64/darwin-modifiers.node',
  ])
  assert.deepEqual(selectPiTuiNativeFiles({ platform: 'linux', arch: 'x64' }), [])
  assert.equal(selectPiTuiNativeFiles({ platform: 'freebsd', arch: 'x64' }), null)

  assert.equal(detectLinuxLibc({ header: { glibcVersionRuntime: '2.39' } }), 'gnu')
  assert.equal(detectLinuxLibc({ sharedObjects: ['/lib/ld-musl-x86_64.so.1'] }), 'musl')
  assert.equal(detectLinuxLibc({ header: {}, sharedObjects: [] }), null)
})

test('SEA runtime pruning retains every native artifact for an unknown target', async () => {
  const runtime = await mkdtemp(join(tmpdir(), 'pisper-sea-unknown-native-'))
  const clipboard = 'node_modules/@earendil-works/pi-coding-agent/node_modules/@mariozechner'
  const piTui =
    'node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-tui/native'
  try {
    await Promise.all([
      createFile(runtime, `${clipboard}/clipboard-win32-x64-msvc/binding.node`),
      createFile(runtime, `${clipboard}/clipboard-darwin-universal/binding.node`),
      createFile(runtime, `${piTui}/win32/prebuilds/win32-x64/binding.node`),
      createFile(runtime, `${piTui}/darwin/prebuilds/darwin-arm64/binding.node`),
    ])
    const result = await pruneRuntime(runtime, { platform: 'freebsd', arch: 'x64', libc: null })
    const native = await collectNativeState(runtime, result.nativeSelection)

    assert.deepEqual(result.nativeSelection, {
      clipboardPackage: null,
      piTuiNativeFiles: null,
    })
    assert.equal(native.pass, true)
    assert.deepEqual(native.retainedClipboardPackages, [
      'clipboard-darwin-universal',
      'clipboard-win32-x64-msvc',
    ])
    assert.equal(native.retainedPiTuiNativeFiles.length, 2)
  } finally {
    await rm(runtime, { recursive: true, force: true })
  }
})

test('SEA runtime pruning preserves runtime src and only the selected native closure', async () => {
  const runtime = await mkdtemp(join(tmpdir(), 'pisper-sea-prune-'))
  const nodeModules = join(runtime, 'node_modules')
  const piNested = 'node_modules/@earendil-works/pi-coding-agent/node_modules/@mariozechner'
  const piTui = 'node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-tui'
  const files = [
    ['node_modules/officeparser/dist/index.mjs', 'export const OfficeParser = {}'],
    ['node_modules/officeparser/dist/officeparser.browser.mjs', 'browser'],
    ['node_modules/officeparser/dist/officeparser.browser.slim.iife.js', 'browser'],
    ['node_modules/protobufjs/src/index.js', 'module.exports = {}'],
    ['node_modules/protobufjs/src/index.ts', 'export {}'],
    ['node_modules/example/dist/index.js', 'module.exports = {}'],
    ['node_modules/example/dist/index.d.ts', 'export {}'],
    ['node_modules/example/dist/index.js.map', '{}'],
    ['node_modules/example/docs/guide.md', 'guide'],
    ['node_modules/example/LICENSE', 'license text'],
    ['node_modules/yargs/locales/en.json', '{}'],
    ['node_modules/yargs/locales/zh_CN.json', '{}'],
    ['node_modules/yargs/locales/de.json', '{}'],
    ['node_modules/zod/v4/locales/index.js', 'export {}'],
    [`${piNested}/clipboard/index.js`, 'module.exports = {}'],
    [`${piNested}/clipboard-win32-x64-msvc/package.json`, '{}'],
    [`${piNested}/clipboard-win32-x64-msvc/clipboard.win32-x64-msvc.node`, 'native'],
    [`${piNested}/clipboard-darwin-universal/package.json`, '{}'],
    [`${piNested}/clipboard-darwin-universal/clipboard.darwin-universal.node`, 'native'],
    [`${piTui}/native/win32/prebuilds/win32-x64/win32-console-mode.node`, 'native'],
    [`${piTui}/native/win32/prebuilds/win32-arm64/win32-console-mode.node`, 'native'],
    [`${piTui}/native/darwin/prebuilds/darwin-x64/darwin-modifiers.node`, 'native'],
  ]

  try {
    await Promise.all(files.map(([path, contents]) => createFile(runtime, path, contents)))
    const result = await pruneRuntime(runtime, { platform: 'win32', arch: 'x64', libc: null })

    assert.equal(await exists(join(nodeModules, 'protobufjs', 'src', 'index.js')), true)
    assert.equal(await exists(join(nodeModules, 'protobufjs', 'src', 'index.ts')), false)
    assert.equal(await exists(join(nodeModules, 'officeparser', 'dist', 'index.mjs')), true)
    assert.equal(
      await exists(join(nodeModules, 'officeparser', 'dist', 'officeparser.browser.mjs')),
      false,
    )
    assert.equal(await exists(join(nodeModules, 'example', 'dist', 'index.js')), true)
    assert.equal(await exists(join(nodeModules, 'example', 'dist', 'index.d.ts')), false)
    assert.equal(await exists(join(nodeModules, 'example', 'docs')), false)
    assert.equal(await exists(join(nodeModules, 'example', 'LICENSE')), true)
    assert.equal(await exists(join(nodeModules, 'yargs', 'locales', 'en.json')), true)
    assert.equal(await exists(join(nodeModules, 'yargs', 'locales', 'zh_CN.json')), true)
    assert.equal(await exists(join(nodeModules, 'yargs', 'locales', 'de.json')), false)
    assert.equal(await exists(join(nodeModules, 'zod', 'v4', 'locales', 'index.js')), true)
    assert.equal(
      await exists(join(runtime, ...`${piNested}/clipboard-win32-x64-msvc`.split('/'))),
      true,
    )
    assert.equal(
      await exists(join(runtime, ...`${piNested}/clipboard-darwin-universal`.split('/'))),
      false,
    )
    assert.equal(
      await exists(
        join(
          runtime,
          ...`${piTui}/native/win32/prebuilds/win32-x64/win32-console-mode.node`.split('/'),
        ),
      ),
      true,
    )
    assert.equal(
      await exists(
        join(
          runtime,
          ...`${piTui}/native/win32/prebuilds/win32-arm64/win32-console-mode.node`.split('/'),
        ),
      ),
      false,
    )
    assert.ok(result.audit.rules.sourceOnly.bytes > 0)
    assert.ok(result.audit.rules.officeparserBrowserBundles.bytes > 0)
    assert.ok(result.audit.rules.foreignClipboardNative.bytes > 0)

    const native = await collectNativeState(runtime, result.nativeSelection)
    assert.equal(native.pass, true)
    assert.deepEqual(native.retainedClipboardPackages, ['clipboard-win32-x64-msvc'])
    assert.deepEqual(native.retainedPiTuiNativeFiles, [
      'win32/prebuilds/win32-x64/win32-console-mode.node',
    ])
  } finally {
    await rm(runtime, { recursive: true, force: true })
  }
})

test('SEA size manifest enforces its runtime budget and executable audit', () => {
  const snapshot = (bytes) => ({
    bytes,
    fileCount: 10,
    directoryCount: 4,
    packageCount: 2,
    topLevel: [],
    packages: [],
  })
  const base = {
    appVersion: '1.2.3',
    target: { platform: 'win32', arch: 'x64', libc: null },
    beforePrune: snapshot(SEA_RUNTIME_BUDGET_BYTES + 1024),
    pruning: { bytes: 1024, fileCount: 1, directoryCount: 0, rules: {} },
    criticalFiles: [{ kind: 'test', path: 'required.js', exists: true, bytes: 1, fileCount: 1 }],
    native: { pass: true },
    generatedAt: '2026-01-01T00:00:00.000Z',
  }
  const passing = createSizeManifest({
    ...base,
    afterPrune: snapshot(SEA_RUNTIME_BUDGET_BYTES),
  })
  assert.equal(passing.schema, 'pisper.sea-runtime-size-manifest')
  assert.equal(passing.version, 1)
  assert.equal(passing.budget.runtimeMiB, 120)
  assert.equal(passing.budget.pass, true)
  assert.equal(passing.pass, true)
  assert.throws(() => assertSizeManifest(passing, { requireExecutable: true }), /executable size/)

  const finalized = finalizeSizeManifest(passing, 42)
  assert.doesNotThrow(() => assertSizeManifest(finalized, { requireExecutable: true }))

  const overBudget = createSizeManifest({
    ...base,
    afterPrune: snapshot(SEA_RUNTIME_BUDGET_BYTES + 1),
  })
  assert.equal(overBudget.pass, false)
  assert.throws(() => assertSizeManifest(overBudget), /exceeds/)
})

test('SEA critical closure audits dynamic packages and reports missing files', async () => {
  const runtime = await mkdtemp(join(tmpdir(), 'pisper-sea-critical-'))
  try {
    await createFile(runtime, 'present.js')
    const inspected = await inspectCriticalFiles(runtime, [
      { kind: 'test', path: 'present.js' },
      { kind: 'test', path: 'missing.js' },
    ])
    assert.deepEqual(
      inspected.map(({ path, exists: fileExists }) => [path, fileExists]),
      [
        ['present.js', true],
        ['missing.js', false],
      ],
    )

    const paths = new Set(criticalRuntimeEntries().map((entry) => entry.path))
    for (const required of [
      'node_modules/officeparser/dist/OfficeParser.js',
      'node_modules/fflate/esm/index.mjs',
      'node_modules/pdfjs-dist/legacy/build/pdf.worker.mjs',
      'node_modules/@modelcontextprotocol/sdk/dist/esm/client/stdio.js',
      'node_modules/playwright-core/index.mjs',
      'node_modules/@earendil-works/pi-coding-agent/dist/core/skills.js',
    ]) {
      assert.equal(paths.has(required), true, required)
    }
  } finally {
    await rm(runtime, { recursive: true, force: true })
  }
})
