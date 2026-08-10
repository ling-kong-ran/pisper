import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  componentsRoot,
  executableName,
  releaseArchitecture,
  releasePlatform,
  supportedTarget,
} from '../../packages/pisper/lib/platform.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'

test('pisper is an isolated private source package exposing only its command', async () => {
  const manifest = JSON.parse(await readFile('packages/pisper/package.json', 'utf8'))

  assert.equal(manifest.name, 'pisper')
  assert.equal(manifest.private, true)
  assert.deepEqual(manifest.bin, { pisper: 'bin/pisper.mjs' })
  assert.match(manifest.pisper.tuiVersion, /^\d+\.\d+\.\d+$/)
  assert.match(manifest.pisper.runtimeVersion, /^\d+\.\d+\.\d+$/)
  assert.deepEqual(releaseComponentsForPath('packages/pisper/lib/install.mjs'), [])
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
})

test('npm publication follows the component release workflow automatically', async () => {
  const [workflow, release, verifier] = await Promise.all([
    readFile('.github/workflows/publish-npm.yml', 'utf8'),
    readFile('scripts/release.mjs', 'utf8'),
    readFile('scripts/verify-release-head.mjs', 'utf8'),
  ])

  assert.match(workflow, /id-token: write/)
  assert.match(workflow, /npm publish release\/npm\/tarballs\/\*\.tgz --access public --provenance/)
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
