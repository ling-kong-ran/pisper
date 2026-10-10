import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repo = fileURLToPath(new URL('../../', import.meta.url))
const preload = new URL('./fixtures/rust-packaging-command-fixture.mjs', import.meta.url).href

async function runPackaging({ key, signingIdentity, target = 'x86_64-pc-windows-msvc' } = {}) {
  const temporaryRoot = await realpath(tmpdir())
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
    await copyFile(
      path.join(repo, 'scripts/stage-rust-runtime.mjs'),
      path.join(root, 'scripts/stage-rust-runtime.mjs'),
    )
    for (const file of ['tauri.conf.json', 'tauri.updater.conf.json', 'updater.pubkey']) {
      await copyFile(path.join(repo, 'src-tauri', file), path.join(root, 'src-tauri', file))
    }
    // 资源准备与编译均是外部边界；入口自身仍真实读写和暂存临时 root 中的文件。
    const suffix = target === 'aarch64-apple-darwin' ? '' : '.exe'
    await put(`release/rust-build/pisper-server${suffix}`, 'synthetic-runtime')
    await put(`src-tui/target/release/pisper${suffix}`, 'synthetic-tui')
    if (!suffix) await put('runtime-rs/target/release/pisper-server', 'synthetic-runtime')
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
    delete env.CI
    delete env.APPLE_SIGNING_IDENTITY
    if (signingIdentity) env.APPLE_SIGNING_IDENTITY = signingIdentity
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
  assert.deepEqual(build.args.slice(1, 4), [
    'build',
    '--bundles',
    target === 'aarch64-apple-darwin' ? 'app,dmg' : 'nsis',
  ])
  const overlay = configs.find(
    ({ file }) => file === path.join(root, 'release/rust-build/tauri-rust.conf.json'),
  )
  assert.ok(overlay, 'Rust resource overlay must reach Tauri')
  assert.equal(overlay.value.bundle.resources['../release/sea/runtime/'], null)
  assert.equal(
    overlay.value.bundle.macOS?.minimumSystemVersion,
    target === 'aarch64-apple-darwin' ? '14.0' : undefined,
  )
  assert.ok(
    commands.some(({ args }) => path.basename(args[0]) === 'smoke-rust-provider-upgrade.mjs'),
  )
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

// 原生 Mac 包同样保留资源、启动与终端验收，不通过假装 Windows 绕过测试。
test('macOS arm64 packaging stages native binaries and App/DMG bundles', async () => {
  const result = await runPackaging({ target: 'aarch64-apple-darwin' })
  assertRustPackagingPreserved(result)
  assert.equal(result.build.ci, 'true')
  assert.equal(result.build.deploymentTarget, '14.0')
  assert.equal(result.configs[0].value.bundle.macOS.signingIdentity, '-')
  assert.ok(result.commands.some(({ command }) => command === 'codesign'))
  assert.ok(
    result.commands
      .filter(
        ({ args }) =>
          args.includes('desktop_terminal::tests') ||
          args.includes('desktop_shell::startup_diagnostics::tests'),
      )
      .every(({ deploymentTarget }) => deploymentTarget === '14.0'),
  )
  assert.ok(
    result.commands
      .filter(({ command }) => command === 'fixture-cargo')
      .every(({ ci }) => ci === undefined),
  )
  assert.equal(result.build.args.includes('--target'), false)
  assert.ok(
    result.commands.some(
      ({ args }) => args.includes('desktop_terminal::tests') && args.includes('--release'),
    ),
  )
})

test('macOS packaging preserves an explicitly configured platform signing identity', async () => {
  const identity = 'Developer ID Application: Synthetic Fixture'
  const result = await runPackaging({ target: 'aarch64-apple-darwin', signingIdentity: identity })
  assert.equal(result.configs[0].value.bundle.macOS.signingIdentity, identity)
  assert.ok(result.commands.some(({ command }) => command === 'codesign'))
})

test('standalone Rust CLI ships the native runtime, frontend and matching component version', async () => {
  const root = await mkdtemp(path.join(await realpath(tmpdir()), 'pisper-cli-package-'))
  try {
    const put = async (name, value) => {
      await mkdir(path.dirname(path.join(root, name)), { recursive: true })
      await writeFile(path.join(root, name), value)
    }
    await put('src-tui/Cargo.toml', '[package]\nname = "pisper"\nversion = "1.2.3"\n')
    await put('src-tauri/desktop-package.json', JSON.stringify({ bundled: { runtime: '4.5.6' } }))
    await put('src-tui/target/release/pisper', 'fixture-cli')
    await put('runtime-rs/target/release/pisper-server', 'fixture-runtime')
    await put(
      'runtime-rs/src/native_image_runtime/telea.rs',
      '/*\nIntel License Agreement\nfixture\n*/\n',
    )
    await put('dist/index.html', '<title>fixture frontend</title>')
    await put(
      'scripts/stage-rust-speech.mjs',
      'export async function stageRustSpeech() { return [] }\n',
    )
    for (const file of ['package-tui.mjs', 'stage-rust-runtime.mjs']) {
      await copyFile(path.join(repo, 'scripts', file), path.join(root, 'scripts', file))
    }
    const result = spawnSync(
      process.execPath,
      ['--import', preload, path.join(root, 'scripts/package-tui.mjs')],
      {
        cwd: root,
        encoding: 'utf8',
        timeout: 15000,
        env: {
          ...process.env,
          CARGO: 'fixture-cargo',
          RUSTC: 'fixture-rustc',
          PISPER_PACKAGING_TEST_TARGET: 'aarch64-apple-darwin',
          PISPER_TUI_STAGE_DIR: path.join(root, 'stage'),
        },
      },
    )
    assert.equal(result.status, 0, result.stderr || result.stdout)
    const manifest = JSON.parse(await readFile(path.join(root, 'stage/manifest.json'), 'utf8'))
    assert.equal(manifest.runtimeVersion, '4.5.6')
    assert.equal(manifest.backend, 'rust')
    assert.equal(await readFile(path.join(root, 'stage/pisper-sidecar'), 'utf8'), 'fixture-runtime')
    assert.match(
      await readFile(path.join(root, 'stage/sidecar-runtime/dist/index.html'), 'utf8'),
      /fixture frontend/,
    )
    const runtime = JSON.parse(
      await readFile(path.join(root, 'stage/sidecar-runtime/package.json'), 'utf8'),
    )
    assert.equal(runtime.version, '4.5.6')
    assert.equal(runtime.executableSha256.length, 64)
    assert.equal(runtime.nativeImageLicense.sha256.length, 64)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
