import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repo = fileURLToPath(new URL('../../', import.meta.url))
const preload = new URL('./fixtures/rust-packaging-command-fixture.mjs', import.meta.url).href

async function runPackaging({ key, target = 'x86_64-pc-windows-msvc' } = {}) {
  const temporaryRoot = path.resolve(tmpdir())
  const root = await mkdtemp(path.join(temporaryRoot, 'pisper-rust-packaging-'))
  try {
    async function put(relative, content) {
      const destination = path.join(root, relative)
      await mkdir(path.dirname(destination), { recursive: true })
      await writeFile(destination, content)
    }
    await put(
      'src-tauri/desktop-package.json',
      JSON.stringify({ version: '1.2.3', bundled: { runtime: '4.5.6', tui: '7.8.9' } }),
    )
    await put(
      'scripts/stage-rust-speech.mjs',
      'export async function stageRustSpeech() { return [] }\n',
    )
    await copyFile(
      path.join(repo, 'scripts/package-tauri-rust.mjs'),
      path.join(root, 'scripts/package-tauri-rust.mjs'),
    )
    for (const file of ['tauri.conf.json', 'tauri.updater.conf.json', 'updater.pubkey']) {
      await copyFile(path.join(repo, 'src-tauri', file), path.join(root, 'src-tauri', file))
    }
    // 资源准备与编译均是外部边界；入口自身仍真实读写和暂存临时 root 中的文件。
    await put('release/rust-build/pisper-server.exe', 'synthetic-runtime')
    await put('src-tui/target/release/pisper.exe', 'synthetic-tui')
    await put(
      'runtime-rs/src/native_image_runtime/telea.rs',
      '/*\nIntel License Agreement\nSynthetic resource fixture\n*/\n',
    )
    const env = {
      ...process.env,
      CARGO: 'fixture-cargo',
      RUSTC: 'fixture-rustc',
      PISPER_PACKAGING_TEST_TARGET: target,
    }
    delete env.TAURI_SIGNING_PRIVATE_KEY
    delete env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    delete env.RUNNER_ENVIRONMENT
    if (key !== undefined) env.TAURI_SIGNING_PRIVATE_KEY = key
    if (key?.trim()) {
      env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = 'synthetic-password'
      env.RUNNER_ENVIRONMENT = 'fixture-ci'
    }
    const result = spawnSync(
      process.execPath,
      ['--import', preload, path.join(root, 'scripts/package-tauri-rust.mjs')],
      { cwd: root, env, encoding: 'utf8', timeout: 15000 },
    )
    if (result.error) throw result.error
    const trace = result.stdout.match(/^RUST_PACKAGING_TEST_COMMANDS=(.+)$/m)
    assert.ok(trace, result.stderr || result.stdout)
    assert.equal(result.status, 0, result.stderr || result.stdout)
    const commands = JSON.parse(trace[1])
    const builds = commands.filter(({ args }) => path.basename(args[0]) === 'tauri.js')
    const stages = commands.filter(
      ({ args }) => path.basename(args[0]) === 'stage-tauri-artifacts.mjs',
    )
    assert.equal(builds.length, 1, 'Full entry must invoke the Tauri build exactly once')
    assert.equal(stages.length, 1, 'Full entry must stage the resulting installer exactly once')
    const build = builds[0]
    const configs = []
    for (let index = 0; index < build.args.length; index++) {
      if (build.args[index] === '--config') {
        const file = build.args[++index]
        configs.push({ file, value: JSON.parse(await readFile(file, 'utf8')) })
      }
    }
    const publicKey = (await readFile(path.join(root, 'src-tauri/updater.pubkey'), 'utf8')).trim()
    return { root, target, commands, build, stage: stages[0], configs, publicKey }
  } finally {
    // 递归清理仅允许本测试 mkdtemp 得到的直接子目录，不触及仓库或真实产物。
    assert.equal(path.dirname(path.resolve(root)), temporaryRoot)
    await rm(root, { recursive: true, force: true })
  }
}

function assertRustPackagingPreserved(result) {
  const { root, target, build, stage, commands, configs } = result
  assert.deepEqual(build.args.slice(1, 4), ['build', '--bundles', 'nsis'])
  const overlay = configs.find(
    ({ file }) => file === path.join(root, 'release/rust-build/tauri-rust.conf.json'),
  )
  assert.ok(overlay, 'Rust resource overlay must reach Tauri')
  assert.equal(overlay.value.bundle.resources['../release/sea/runtime/'], null)
  assert.equal(overlay.value.bundle.resources['../release/rust-runtime/'], 'sidecar-runtime/')
  const targetArgs = target.endsWith('-gnu') ? [target] : []
  assert.equal(
    stage.bundleDir,
    path.join(root, 'src-tauri/target', ...targetArgs, 'release/bundle'),
  )
  assert.equal(stage.stageDir, path.join(root, 'release/tauri-rust-artifacts'))
  assert.ok(commands.some(({ command, args }) => command === 'fixture-cargo' && args[0] === 'test'))
  assert.ok(commands.some(({ args }) => path.basename(args[0]) === 'smoke-rust-usability.mjs'))
  const buildAt = commands.indexOf(build)
  assert.ok(
    commands.findIndex(({ args }) => path.basename(args[0]) === 'smoke-rust-usability.mjs') <
      buildAt,
  )
  assert.ok(commands.indexOf(stage) > buildAt)
}

for (const target of ['x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu']) {
  test(`signed Rust packaging enables canonical updater artifacts and requires signatures (${target})`, async () => {
    const result = await runPackaging({ key: 'synthetic-signing-key', target })
    assertRustPackagingPreserved(result)
    const updater = result.configs.find(
      ({ file }) => file === path.join(result.root, 'src-tauri/tauri.updater.conf.json'),
    )
    assert.deepEqual(
      {
        canonicalUpdater: Boolean(updater),
        requireSignature: result.stage.args.includes('--require-signature'),
      },
      { canonicalUpdater: true, requireSignature: true },
      'Signed builds must enable updater artifacts and require their signatures together',
    )
    assert.equal(updater.value.bundle.createUpdaterArtifacts, true)
    assert.equal(updater.value.plugins.updater.pubkey, result.publicKey)
    assert.ok(result.stage.args.includes('--require-signature'))
    assert.equal(result.build.keyPresent, true)
    assert.equal(result.build.passwordPresent, true)
    if (target.endsWith('-gnu')) assert.deepEqual(result.build.args.slice(-2), ['--target', target])
  })
}

for (const key of [undefined, ' \t ']) {
  test(`unsigned local Rust packaging is preserved for ${key === undefined ? 'absent' : 'blank'} signing key`, async () => {
    const result = await runPackaging({ key })
    assertRustPackagingPreserved(result)
    assert.equal(result.configs.length, 1)
    assert.equal(result.stage.args.includes('--require-signature'), false)
    assert.equal(result.build.keyPresent, false)
  })
}
