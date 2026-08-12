import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { c as createTar } from 'tar'
import {
  acquireInstallLock,
  ensurePisperInstallation,
  extractComponentArchive,
} from '../../packages/pisper/lib/install.mjs'
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
import {
  handleNpmHelp,
  handleNpmUpdate,
  parseNpmHelpRequest,
  parseNpmUpdateRequest,
} from '../../packages/pisper/lib/npm-update.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'

test('pisper is an isolated private source package exposing only its command', async () => {
  const manifest = JSON.parse(await readFile('packages/pisper/package.json', 'utf8'))

  assert.equal(manifest.name, 'pisper')
  assert.equal(manifest.private, true)
  assert.deepEqual(manifest.bin, { pisper: 'bin/pisper.mjs' })
  assert.match(manifest.pisper.tuiVersion, /^\d+\.\d+\.\d+$/)
  assert.match(manifest.pisper.runtimeVersion, /^\d+\.\d+\.\d+$/)
  assert.equal(manifest.scripts?.postinstall, 'node lib/postinstall.mjs')
  for (const path of [
    'packages/pisper/bin/pisper.mjs',
    'packages/pisper/lib/install.mjs',
    'packages/pisper/lib/npm-update.mjs',
    'packages/pisper/lib/postinstall.mjs',
  ]) {
    assert.deepEqual(releaseComponentsForPath(path), ['runtime'])
  }
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

test('npm installer uses native tar with a Node fallback', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-npm-tar-'))
  const source = join(root, 'source')
  const archive = join(root, 'component.tar.gz')
  const destination = join(root, 'destination')
  t.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(source, 'payload'), { recursive: true })
  await writeFile(join(source, 'payload', 'manifest.json'), '{"version":"test"}\n')
  await createTar({ cwd: source, file: archive, gzip: true }, ['payload'])

  await mkdir(destination)
  await extractComponentArchive(archive, destination)
  assert.equal(await readFile(join(destination, 'manifest.json'), 'utf8'), '{"version":"test"}\n')

  await rm(destination, { recursive: true, force: true })
  await mkdir(destination)
  await extractComponentArchive(archive, destination, join(root, 'missing-tar'))
  assert.equal(await readFile(join(destination, 'manifest.json'), 'utf8'), '{"version":"test"}\n')
})

test('npm update is one complete registry update without component selection', async () => {
  assert.equal(parseNpmHelpRequest(['--help']), true)
  assert.equal(parseNpmHelpRequest(['help']), true)
  assert.equal(parseNpmHelpRequest(['help', 'web']), false)
  const help = []
  assert.equal(handleNpmHelp(['--help'], { log: (value) => help.push(value) }), true)
  assert.match(help.join('\n'), /pisper update \[--check\]/)

  assert.deepEqual(parseNpmUpdateRequest(['update']), { checkOnly: false, help: false })
  assert.deepEqual(parseNpmUpdateRequest(['update', '--check']), {
    checkOnly: true,
    help: false,
  })
  assert.deepEqual(parseNpmUpdateRequest(['help', 'update']), { checkOnly: false, help: true })
  assert.equal(parseNpmUpdateRequest(['doctor']), null)
  assert.throws(() => parseNpmUpdateRequest(['update', 'runtime']), /do not accept component names/)

  const logs = []
  let installs = 0
  const handled = await handleNpmUpdate(['update', '--check'], {
    currentVersion: '1.2.3',
    queryLatest: async () => '1.2.4',
    installLatest: async () => {
      installs += 1
    },
    log: (value) => logs.push(value),
  })
  assert.equal(handled, true)
  assert.equal(installs, 0)
  assert.match(logs.join('\n'), /1\.2\.3 -> 1\.2\.4/)

  await handleNpmUpdate(['update'], {
    currentVersion: '1.2.3',
    queryLatest: async () => '1.2.4',
    installLatest: async () => {
      installs += 1
    },
    log: (value) => logs.push(value),
  })
  assert.equal(installs, 1)
})

test('npm launcher prepares signed components and serves its bundled Web frontend', async () => {
  const [installer, launcher, postinstall] = await Promise.all([
    readFile('packages/pisper/lib/install.mjs', 'utf8'),
    readFile('packages/pisper/bin/pisper.mjs', 'utf8'),
    readFile('packages/pisper/lib/postinstall.mjs', 'utf8'),
  ])

  assert.match(installer, /component === 'tui' \? 'TUI_Component' : 'Runtime_Node'/)
  assert.match(installer, /`Pisper_\$\{label\}_\$\{version\}_\$\{target\}\.tar\.gz`/)
  assert.match(installer, /component: 'runtime'/)
  assert.match(installer, /sidecar-runtime', 'package\.json'/)
  assert.match(installer, /resolvePlatformBundle/)
  assert.match(installer, /process\.platform === 'win32' \? destination : staging/)
  assert.doesNotMatch(installer, /\bfetch\(|github\.com|PISPER_CLI_DOWNLOAD/)
  assert.doesNotMatch(installer, /\bcopyFile\b|\bcp\(/)
  assert.match(launcher, /PISPER_RUNTIME_NODE: process\.execPath/)
  assert.doesNotMatch(launcher, /PISPER_SIDECAR_PATH/)
  assert.match(launcher, /PISPER_APP_ROOT: installation\.appRoot/)
  assert.match(launcher, /PISPER_FRONTEND_ROOT: frontendRoot/)
  assert.match(launcher, /handleNpmHelp/)
  assert.match(launcher, /handleNpmUpdate/)
  assert.match(postinstall, /ensurePisperInstallation/)
  assert.doesNotMatch(postinstall, /\bfetch\(|github\.com/)
})

test('npm installation uses an isolated Node Runtime without a SEA executable', async () => {
  const installRoot = await mkdtemp(join(tmpdir(), 'pisper-node-runtime-'))
  const previousInstallRoot = process.env.PISPER_NPM_INSTALL_DIR
  process.env.PISPER_NPM_INSTALL_DIR = installRoot
  const manifest = JSON.parse(await readFile('packages/pisper/package.json', 'utf8'))
  const root = componentsRoot()
  const tuiDestination = join(root, 'npm', 'versions', manifest.pisper.tuiVersion)
  const runtimeDestination = join(root, 'npm-runtime', 'versions', manifest.pisper.runtimeVersion)
  const runtimeRoot = join(runtimeDestination, 'sidecar-runtime')

  try {
    await Promise.all([
      mkdir(tuiDestination, { recursive: true }),
      mkdir(join(runtimeRoot, 'runtime'), { recursive: true }),
    ])
    await Promise.all([
      writeFile(join(tuiDestination, executableName()), 'test TUI'),
      writeFile(
        join(tuiDestination, 'manifest.json'),
        JSON.stringify({
          version: manifest.pisper.tuiVersion,
          platform: releasePlatform(),
          arch: releaseArchitecture(),
          command: executableName(),
        }),
      ),
      writeFile(join(runtimeRoot, 'package.json'), '{}'),
      writeFile(join(runtimeRoot, 'runtime', 'sidecar.mjs'), '// test Runtime'),
      writeFile(
        join(runtimeDestination, 'manifest.json'),
        JSON.stringify({
          version: manifest.pisper.runtimeVersion,
          platform: releasePlatform(),
          arch: releaseArchitecture(),
          command: 'sidecar-runtime/runtime/sidecar.mjs',
        }),
      ),
    ])

    const installation = await ensurePisperInstallation()
    assert.equal(installation.executable, join(tuiDestination, executableName()))
    assert.equal(installation.appRoot, runtimeRoot)
    assert.equal('sidecar' in installation, false)
  } finally {
    if (previousInstallRoot === undefined) delete process.env.PISPER_NPM_INSTALL_DIR
    else process.env.PISPER_NPM_INSTALL_DIR = previousInstallRoot
    await rm(installRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
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
  assert.match(workflow, /--ignore-scripts/)
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
