import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { acquireInstallLock } from '../../packages/pisper/lib/install.mjs'
import {
  componentsRoot,
  executableName,
  releaseArchitecture,
  releasePlatform,
  supportedTarget,
} from '../../packages/pisper/lib/platform.mjs'
import {
  npmPlatformAlias,
  npmPlatformOptionalDependencies,
  npmPlatformVersion,
} from '../../packages/pisper/lib/npm-platform.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'

test('pisper is an isolated private source package exposing only its command', async () => {
  const manifest = JSON.parse(await readFile('packages/pisper/package.json', 'utf8'))

  assert.equal(manifest.name, 'pisper')
  assert.equal(manifest.private, true)
  assert.deepEqual(manifest.bin, { pisper: 'bin/pisper.mjs' })
  assert.match(manifest.pisper.tuiVersion, /^\d+\.\d+\.\d+$/)
  assert.match(manifest.pisper.runtimeVersion, /^\d+\.\d+\.\d+$/)
  assert.equal(manifest.scripts?.postinstall, undefined)
  assert.deepEqual(releaseComponentsForPath('packages/pisper/lib/install.mjs'), ['runtime'])
  assert.deepEqual(releaseComponentsForPath('scripts/package-npm-platforms.mjs'), [])
})

test('npm installer replaces a stale legacy lock instead of waiting silently', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-npm-lock-'))
  const lockPath = join(root, 'runtime-install.lock')
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(lockPath, '')
  const stale = new Date(Date.now() - 11 * 60 * 1000)
  await utimes(lockPath, stale, stale)

  const lock = await acquireInstallLock(lockPath, async () => false)
  assert.ok(lock)
  try {
    const metadata = JSON.parse(await readFile(lockPath, 'utf8'))
    assert.equal(metadata.pid, process.pid)
    assert.ok(Number.isSafeInteger(metadata.createdAt))
  } finally {
    await lock.close()
  }
})

test('npm launcher installs signed TUI and Runtime components without duplicating Runtime', async () => {
  const [installer, launcher] = await Promise.all([
    readFile('packages/pisper/lib/install.mjs', 'utf8'),
    readFile('packages/pisper/bin/pisper.mjs', 'utf8'),
  ])

  assert.match(installer, /'TUI_Component'/)
  assert.match(installer, /`Pisper_\$\{label\}_\$\{version\}_\$\{target\}\.tar\.gz`/)
  assert.match(installer, /component: 'runtime'/)
  assert.match(installer, /sidecar-runtime', 'package\.json'/)
  assert.match(installer, /resolvePlatformBundle/)
  assert.match(installer, /process\.platform === 'win32' \? destination : staging/)
  assert.doesNotMatch(installer, /\bfetch\(|github\.com|PISPER_CLI_DOWNLOAD/)
  assert.doesNotMatch(installer, /\bcopyFile\b|\bcp\(/)
  assert.match(launcher, /PISPER_SIDECAR_PATH: installation\.sidecar/)
  assert.match(launcher, /PISPER_APP_ROOT: installation\.appRoot/)
})

test('npm platform mapping matches signed component release assets', () => {
  assert.equal(releasePlatform('win32'), 'windows')
  assert.equal(releasePlatform('darwin'), 'darwin')
  assert.equal(releaseArchitecture('x64'), 'x86_64')
  assert.equal(releaseArchitecture('arm64'), 'aarch64')
  assert.equal(supportedTarget('linux', 'x64'), 'linux_x86_64')
  assert.equal(executableName('win32'), 'pisper.exe')
  assert.match(
    componentsRoot('linux', { PISPER_NPM_INSTALL_DIR: '/tmp/pisper-test' }),
    /components$/,
  )
  assert.throws(() => supportedTarget('linux', 'arm64'), /does not publish a TUI package/)
  assert.equal(npmPlatformAlias('win32', 'x64'), 'pisper-binary-win32-x64')
  assert.equal(npmPlatformVersion('1.2.3', 'darwin', 'arm64'), '1.2.3-darwin-arm64.0')
  assert.deepEqual(npmPlatformOptionalDependencies('1.2.3'), {
    'pisper-binary-win32-x64': 'npm:pisper@1.2.3-win32-x64.0',
    'pisper-binary-darwin-x64': 'npm:pisper@1.2.3-darwin-x64.0',
    'pisper-binary-darwin-arm64': 'npm:pisper@1.2.3-darwin-arm64.0',
    'pisper-binary-linux-x64': 'npm:pisper@1.2.3-linux-x64.0',
  })
})

test('npm publication follows the component release workflow automatically', async () => {
  const [workflow, release, verifier] = await Promise.all([
    readFile('.github/workflows/publish-npm.yml', 'utf8'),
    readFile('scripts/release.mjs', 'utf8'),
    readFile('scripts/verify-release-head.mjs', 'utf8'),
  ])

  assert.match(workflow, /id-token: write/)
  assert.match(workflow, /node scripts\/package-npm-platforms\.mjs/)
  assert.match(workflow, /publish_platform win32-x64/)
  assert.match(workflow, /npm publish "release\/npm\/tarballs\/pisper-\$NPM_VERSION\.tgz"/)
  assert.doesNotMatch(workflow, /PISPER_CLI_SKIP_INSTALL/)
  assert.doesNotMatch(workflow, /NPM_TOKEN|NODE_AUTH_TOKEN/)
  assert.match(workflow, /npm view "pisper@\$NPM_VERSION"/)
  assert.match(workflow, /chore\(release-npm\): \$NPM_TAG/)
  assert.match(release, /publish_npm=true/)
  assert.match(release, /tui_version=\$\{npmTuiVersion\}/)
  assert.match(release, /runtime_version=\$\{npmRuntimeVersion\}/)
  assert.doesNotMatch(release, /--publish-npm/)
  assert.doesNotMatch(release, /--npm-version/)
  assert.match(verifier, /packages\/pisper\/package\.json/)
})
