import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { RUST_DESKTOP_PLATFORMS, resolveRustDesktopPlatform } from '../rust-desktop-platforms.mjs'
import { stageRustSpeech } from '../stage-rust-speech.mjs'

const supported = [
  ['win32', 'x64', 'x86_64-pc-windows-msvc', 'nsis'],
  ['darwin', 'arm64', 'aarch64-apple-darwin', 'app,dmg'],
  ['darwin', 'x64', 'x86_64-apple-darwin', 'app'],
  ['linux', 'x64', 'x86_64-unknown-linux-gnu', 'appimage,deb'],
]

test('desktop artifact plan covers the four native hosts and Windows offline installer', () => {
  assert.equal(Object.keys(RUST_DESKTOP_PLATFORMS).length, supported.length)
  for (const [platform, arch, rustTarget, bundles] of supported) {
    const result = resolveRustDesktopPlatform({ platform, arch })
    assert.equal(result.rustTarget, rustTarget)
    assert.equal(result.bundles, bundles)
    assert.equal(Boolean(result.windowsOffline), platform === 'win32')
    assert.equal(Boolean(result.simpleIntelMacDmg), platform === 'darwin' && arch === 'x64')
    assert.deepEqual(result.targetArgs, [])
  }
  assert.equal(
    resolveRustDesktopPlatform({ platform: 'darwin', arch: 'arm64' }).minimumSystemVersion,
    '14.0',
  )
  assert.equal(
    resolveRustDesktopPlatform({ platform: 'darwin', arch: 'x64' }).minimumSystemVersion,
    '15.5',
  )
})

test('Windows GNU host retains its explicit Tauri target and canonical artifact identity', () => {
  const result = resolveRustDesktopPlatform({
    platform: 'win32',
    arch: 'x64',
    rustTarget: 'x86_64-pc-windows-gnu',
  })
  assert.deepEqual(result.targetArgs, ['--target', 'x86_64-pc-windows-gnu'])
  assert.equal(result.assetTarget, 'windows-x86_64')
})

test('unknown architectures and mismatched Rust toolchains fail before compilation', () => {
  for (const [platform, arch] of [
    ['linux', 'arm64'],
    ['win32', 'arm64'],
    ['darwin', 'ia32'],
    ['freebsd', 'x64'],
  ])
    assert.throws(() => resolveRustDesktopPlatform({ platform, arch }), /Unsupported/)
  for (const [platform, arch, rustTarget] of [
    ['darwin', 'arm64', 'x86_64-apple-darwin'],
    ['win32', 'x64', 'aarch64-pc-windows-msvc'],
    ['linux', 'x64', 'x86_64-unknown-linux-musl'],
  ])
    assert.throws(
      () => resolveRustDesktopPlatform({ platform, arch, rustTarget }),
      /does not match/,
    )
})

async function withFixture(run) {
  const temporary = await mkdtemp(path.join(await realpath(tmpdir()), 'pisper-desktop-resources-'))
  try {
    const root = path.join(temporary, 'source')
    const targetDir = path.join(temporary, 'stage')
    await mkdir(root)
    await mkdir(targetDir)
    const sentinel = path.join(targetDir, 'existing-resource.txt')
    await writeFile(sentinel, 'preserve')
    await run({ temporary, root, targetDir })
    assert.equal(await readFile(sentinel, 'utf8'), 'preserve')
  } finally {
    await rm(temporary, { recursive: true, force: true })
  }
}

for (const [platform, arch] of supported) {
  test(`speech staging refuses corrupted ${platform}-${arch} libraries before writing`, async () => {
    await withFixture(async ({ root, targetDir }) => {
      const name = `sherpa-onnx-${platform === 'win32' ? 'win' : platform}-${arch}`
      const directory = path.join(root, 'node_modules', name)
      await mkdir(directory, { recursive: true })
      await writeFile(
        path.join(directory, 'package.json'),
        JSON.stringify({ name, version: '1.13.7', os: [platform], cpu: [arch] }),
      )
      const filename =
        platform === 'win32'
          ? 'sherpa-onnx-c-api.dll'
          : `libsherpa-onnx-c-api.${platform === 'darwin' ? 'dylib' : 'so'}`
      await writeFile(path.join(directory, filename), 'corrupted pinned library')
      await assert.rejects(
        stageRustSpeech({ root, targetDir, platform, arch }),
        /Pinned speech resource size or SHA256 mismatch/,
      )
      await assert.rejects(readFile(path.join(targetDir, 'speech-native/resources.json')), {
        code: 'ENOENT',
      })
    })
  })
}

test('speech staging rejects foreign package metadata and unsupported platforms', async () => {
  await withFixture(async ({ root, targetDir }) => {
    const directory = path.join(root, 'node_modules/sherpa-onnx-linux-x64')
    await mkdir(directory, { recursive: true })
    await writeFile(
      path.join(directory, 'package.json'),
      JSON.stringify({
        name: 'sherpa-onnx-linux-x64',
        version: '1.13.7',
        os: ['darwin'],
        cpu: ['x64'],
      }),
    )
    await assert.rejects(
      stageRustSpeech({ root, targetDir, platform: 'linux', arch: 'x64' }),
      /Native speech source must be/,
    )
    await assert.rejects(
      stageRustSpeech({ root, targetDir, platform: 'linux', arch: 'arm64' }),
      /Unsupported Rust desktop platform/,
    )
  })
})

test('speech staging refuses a linked source root without writing outside it', async () => {
  await withFixture(async ({ temporary, root, targetDir }) => {
    const linked = path.join(temporary, 'linked-source')
    await symlink(root, linked, process.platform === 'win32' ? 'junction' : 'dir')
    await assert.rejects(
      stageRustSpeech({ root: linked, targetDir, platform: 'linux', arch: 'x64' }),
      /Speech resource path is a link/,
    )
  })
})
