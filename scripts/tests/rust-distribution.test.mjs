import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import {
  copyFile,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { c as createTar } from 'tar'
import {
  npmPlatformAlias,
  npmPlatformVersion,
  npmRuntimeAsset,
  npmRuntimeBackend,
} from '../../packages/pisper/lib/npm-platform.mjs'
import {
  releaseArchitecture,
  releasePlatform,
  supportedTarget,
} from '../../packages/pisper/lib/platform.mjs'

const repo = fileURLToPath(new URL('../../', import.meta.url))
const version = '1.2.3'
const platform = releasePlatform()
const arch = releaseArchitecture()
const target = supportedTarget()
const suffix = process.platform === 'win32' ? '.exe' : ''
const cli = join(repo, 'node_modules/@tauri-apps/cli/tauri.js')

test('Runtime archive identity explicitly separates Node and Rust for all supported targets', () => {
  for (const value of ['windows_x86_64', 'darwin_x86_64', 'darwin_aarch64', 'linux_x86_64']) {
    assert.equal(
      npmRuntimeAsset(version, value, 'rust'),
      `Pisper_Runtime_Rust_${version}_${value}.tar.gz`,
    )
    assert.equal(npmRuntimeAsset(version, value), `Pisper_Runtime_Node_${version}_${value}.tar.gz`)
  }
  assert.equal(npmRuntimeBackend(), 'node', 'Old launcher metadata remains Node-compatible')
  assert.throws(() => npmRuntimeBackend('unknown'), /Unsupported/)
  assert.throws(() => npmRuntimeAsset(version, 'linux_aarch64', 'rust'), /Unsupported/)
})

async function fixture(fn) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-distribution-'))
  const put = async (path, content) => {
    const destination = join(root, path)
    await mkdir(dirname(destination), { recursive: true })
    await writeFile(destination, content)
    return destination
  }
  try {
    await mkdir(join(root, 'node_modules'), { recursive: true })
    for (const name of ['tar', '@threema', '@tauri-apps']) {
      await symlink(
        join(repo, 'node_modules', name),
        join(root, 'node_modules', name),
        process.platform === 'win32' ? 'junction' : 'dir',
      )
    }
    await put('package.json', JSON.stringify({ version, type: 'module' }))
    await mkdir(join(root, 'scripts'))
    for (const name of [
      'archive-component-release.mjs',
      'release-components.mjs',
      'verify-tauri-signature.mjs',
    ]) {
      await copyFile(join(repo, 'scripts', name), join(root, 'scripts', name))
    }
    const key = join(root, 'fixture.key')
    execFileSync(
      process.execPath,
      [cli, 'signer', 'generate', '--ci', '-p', 'fixture-only', '-w', key],
      { stdio: 'pipe' },
    )
    const signingEnv = {
      ...process.env,
      TAURI_SIGNING_PRIVATE_KEY: await readFile(key, 'utf8'),
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'fixture-only',
    }
    await put('src-tauri/updater.pubkey', await readFile(`${key}.pub`))
    await fn({ root, put, signingEnv })
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}

function runArchive(root, component, env) {
  return spawnSync(
    process.execPath,
    [join(root, 'scripts/archive-component-release.mjs'), component, '--require-signature'],
    { cwd: root, env, encoding: 'utf8' },
  )
}

test('Rust component packaging produces signed native Runtime and finds the actual Darwin CLI stage', async () => {
  await fixture(async ({ root, put, signingEnv }) => {
    await put('runtime-rs/Cargo.toml', '[package]\nname="fixture"\n')
    await put('src-tui/Cargo.toml', `[package]\nname="pisper-tui"\nversion="${version}"\n`)
    const stage = `release/rust-runtime-component`
    await put(`${stage}/pisper-sidecar${suffix}`, 'fixture-runtime')
    await put(`${stage}/sidecar-runtime/package.json`, JSON.stringify({ version, backend: 'rust' }))
    await put(
      `${stage}/manifest.json`,
      JSON.stringify({
        version,
        backend: 'rust',
        platform,
        arch,
        command: `pisper-sidecar${suffix}`,
      }),
    )
    const tuiStage = `release/tui/pisper-${version}-${platform}-${arch}`
    await put(`${tuiStage}/pisper${suffix}`, 'fixture-tui')
    for (const component of ['runtime', 'tui']) {
      const result = runArchive(root, component, signingEnv)
      assert.equal(result.status, 0, result.stderr)
    }
    const names = await readdir(join(root, 'release/component-artifacts'))
    for (const name of [
      npmRuntimeAsset(version, target, 'rust'),
      `Pisper_TUI_${version}_${target}.tar.gz`,
      `Pisper_TUI_Component_${version}_${target}.tar.gz`,
    ]) {
      assert.ok(names.includes(name))
      assert.ok(names.includes(`${name}.sig`))
    }
    assert.equal(names.length, 6, 'Rust does not publish a misleading Node payload')
    const unsignedEnv = { ...signingEnv }
    delete unsignedEnv.TAURI_SIGNING_PRIVATE_KEY
    const rejected = runArchive(root, 'runtime', unsignedEnv)
    assert.notEqual(rejected.status, 0)
    assert.match(rejected.stderr, /required for component releases/)
  })
})

for (const backend of ['node', 'rust']) {
  test(`npm installer verifies, installs and reuses the ${backend} payload without switching backends`, async () => {
    await fixture(async ({ root, put, signingEnv }) => {
      const packageRoot = join(root, 'packages/pisper')
      const metadata = {
        version,
        pisper: {
          tuiVersion: version,
          runtimeVersion: version,
          ...(backend === 'rust' ? { runtimeBackend: backend } : {}),
        },
      }
      await put('packages/pisper/package.json', JSON.stringify(metadata))
      await cp(join(repo, 'packages/pisper/lib'), join(packageRoot, 'lib'), { recursive: true })
      await cp(join(repo, 'packages/pisper/bin'), join(packageRoot, 'bin'), { recursive: true })
      await put(
        'packages/pisper/updater.pubkey',
        await readFile(join(root, 'src-tauri/updater.pubkey')),
      )
      const bundle = join(root, 'node_modules', npmPlatformAlias())
      await mkdir(bundle)
      await writeFile(
        join(bundle, 'package.json'),
        JSON.stringify({
          name: 'pisper',
          version: npmPlatformVersion(version),
          pisperBundle: {
            target,
            tuiVersion: version,
            runtimeVersion: version,
            ...(backend === 'rust' ? { runtimeBackend: backend } : {}),
          },
        }),
      )
      for (const component of ['tui', 'runtime']) {
        const command =
          component === 'tui'
            ? `pisper${suffix}`
            : backend === 'rust'
              ? `pisper-sidecar${suffix}`
              : 'sidecar-runtime/runtime/sidecar.mjs'
        const payload = join(root, `payload-${component}`)
        await put(
          `payload-${component}/manifest.json`,
          JSON.stringify({
            version,
            platform,
            arch,
            command,
            ...(backend === 'rust' ? { backend } : {}),
          }),
        )
        await put(
          `payload-${component}/${command}`,
          component === 'tui' && process.platform !== 'win32'
            ? '#!/bin/sh\nprintf "NODE=%s\\nSIDECAR=%s\\n" "$PISPER_RUNTIME_NODE" "$PISPER_SIDECAR_PATH"\n'
            : 'fixture-binary',
        )
        if (component === 'runtime')
          await put(
            `payload-${component}/sidecar-runtime/package.json`,
            JSON.stringify({ version, backend }),
          )
        const name =
          component === 'tui'
            ? `Pisper_TUI_Component_${version}_${target}.tar.gz`
            : npmRuntimeAsset(version, target, backend)
        const artifact = join(bundle, 'components', component, name)
        await mkdir(dirname(artifact), { recursive: true })
        await createTar({ cwd: root, file: artifact, gzip: true, portable: true }, [
          basename(payload),
        ])
        execFileSync(process.execPath, [cli, 'signer', 'sign', artifact], {
          env: signingEnv,
          stdio: 'pipe',
        })
      }
      const env = { ...process.env, PISPER_NPM_INSTALL_DIR: join(root, 'installed') }
      const probe = [
        '--input-type=module',
        '-e',
        `const m=await import(${JSON.stringify(new URL(`file://${packageRoot}/lib/install.mjs`).href)}); console.log('RESULT='+JSON.stringify(await m.ensurePisperInstallation()))`,
      ]
      const run = () => spawnSync(process.execPath, probe, { env, encoding: 'utf8' })
      const first = run()
      assert.equal(first.status, 0, first.stderr)
      const result = JSON.parse(first.stdout.match(/RESULT=(.+)/)[1])
      assert.equal(result.runtimeBackend, backend)
      assert.equal(Boolean(result.sidecar), backend === 'rust')
      assert.ok(result.appRoot.endsWith('sidecar-runtime'))
      const second = run()
      assert.equal(second.status, 0, second.stderr)
      assert.equal(second.stdout.includes('installing'), false)
      if (process.platform !== 'win32') {
        const launch = spawnSync(
          process.execPath,
          [join(packageRoot, 'bin/pisper.mjs'), '--version'],
          {
            env: {
              ...env,
              PISPER_RUNTIME_NODE: 'stale-node',
              PISPER_SIDECAR_PATH: 'stale-sidecar',
            },
            encoding: 'utf8',
          },
        )
        assert.equal(launch.status, 0, launch.stderr)
        assert.ok(
          launch.stdout.includes(backend === 'rust' ? 'NODE=\n' : `NODE=${process.execPath}\n`),
        )
        assert.ok(
          launch.stdout.includes(backend === 'rust' ? `SIDECAR=${result.sidecar}\n` : 'SIDECAR=\n'),
        )
      }
      await rm(join(root, 'installed'), { recursive: true, force: true })
      const runtimeArchive = join(
        bundle,
        'components/runtime',
        npmRuntimeAsset(version, target, backend),
      )
      await writeFile(runtimeArchive, 'tampered payload')
      const rejected = run()
      assert.notEqual(rejected.status, 0)
      assert.match(rejected.stderr, /signature verification failed/)
    })
  })
}

test('npm release staging records the selected backend without changing component targets', async () => {
  for (const backend of ['rust', 'node']) {
    await fixture(async ({ root, put }) => {
      for (const name of ['stage-npm-release-version.mjs', 'release-policy.mjs']) {
        await copyFile(join(repo, 'scripts', name), join(root, 'scripts', name))
      }
      await put(
        'packages/pisper/package.json',
        JSON.stringify({
          version: '1.2.3',
          pisper: { tuiVersion: '1.2.3', runtimeVersion: '1.2.3' },
        }),
      )
      await put(backend === 'rust' ? 'runtime-rs/Cargo.toml' : 'runtime/index.mjs', 'fixture')
      const git = (args) => execFileSync('git', args, { cwd: root, stdio: 'pipe' })
      git(['init'])
      git(['add', '.'])
      git([
        '-c',
        'user.name=Fixture',
        '-c',
        'user.email=fixture@example.test',
        '-c',
        'commit.gpgsign=false',
        'commit',
        '-m',
        'fix: native distribution fixture',
      ])
      const result = spawnSync(
        process.execPath,
        [join(root, 'scripts/stage-npm-release-version.mjs'), '1.2.4', '1.2.5', '1.2.6'],
        { cwd: root, encoding: 'utf8' },
      )
      assert.equal(result.status, 0, result.stderr)
      const manifest = JSON.parse(
        await readFile(join(root, 'packages/pisper/package.json'), 'utf8'),
      )
      assert.equal(manifest.pisper.runtimeBackend, backend)
      assert.equal(manifest.pisper.tuiVersion, '1.2.5')
      assert.equal(manifest.pisper.runtimeVersion, '1.2.6')
    })
  }
})

test('desktop release requires installers, offline installer, updates and frontend components for every platform', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-desktop-assets-'))
  try {
    const names = [
      'latest.json',
      `Pisper_${version}_windows_x86_64-setup.exe`,
      `Pisper_${version}_windows_x86_64-setup.exe.sig`,
      `Pisper_${version}_windows_x86_64-offline-setup.exe`,
      `Pisper_${version}_windows_x86_64-offline-setup.exe.sig`,
      ...['aarch64', 'x86_64'].flatMap((arch) => [
        `Pisper_${version}_darwin_${arch}.dmg`,
        `Pisper_${version}_darwin_${arch}.app.tar.gz`,
        `Pisper_${version}_darwin_${arch}.app.tar.gz.sig`,
      ]),
      `Pisper_${version}_linux_x86_64.AppImage`,
      `Pisper_${version}_linux_x86_64.AppImage.sig`,
      `Pisper_${version}_linux_x86_64.deb`,
      ...['windows_x86_64', 'darwin_aarch64', 'darwin_x86_64', 'linux_x86_64'].flatMap((target) => [
        `Pisper_Desktop_${version}_${target}.tar.gz`,
        `Pisper_Desktop_${version}_${target}.tar.gz.sig`,
      ]),
    ]
    for (const name of names) await writeFile(join(root, name), 'fixture')
    const validate = () =>
      spawnSync(
        process.execPath,
        [join(repo, 'scripts/validate-tauri-release-assets.mjs'), `v${version}`, root],
        { encoding: 'utf8' },
      )
    assert.equal(validate().status, 0)
    await rm(join(root, `Pisper_${version}_darwin_x86_64.dmg`))
    assert.match(validate().stderr, /Missing release assets.*darwin_x86_64/)
    await writeFile(join(root, `Pisper_${version}_darwin_x86_64.dmg`), 'fixture')
    await rm(join(root, `Pisper_Desktop_${version}_linux_x86_64.tar.gz.sig`))
    assert.match(validate().stderr, /Missing release assets.*Desktop.*linux_x86_64/)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
