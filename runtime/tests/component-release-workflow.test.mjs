import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  componentReleasePaths,
  componentReleaseSubjects,
  detectReleaseComponents,
  releaseComponentsForPath,
} from '../../scripts/release-changes.mjs'
import {
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
  releaseVersionFromTag,
} from '../../scripts/release-components.mjs'

test('desktop, TUI, and runtime own independent versions and tags', async () => {
  const root = process.cwd()
  const [desktop, tui, runtime] = await Promise.all([
    readComponentVersion(root, 'desktop'),
    readComponentVersion(root, 'tui'),
    readComponentVersion(root, 'runtime'),
  ])

  for (const version of [desktop, tui, runtime]) {
    assert.match(version, /^\d+\.\d+\.\d+$/)
  }
  assert.equal(releaseTag('desktop', '1.2.3'), 'v1.2.3')
  assert.equal(releaseTag('tui', '1.2.3'), 'tui-v1.2.3')
  assert.equal(releaseTag('runtime', '1.2.3'), 'runtime-v1.2.3')
  assert.equal(releaseVersionFromTag('tui', 'tui-v1.2.3'), '1.2.3')
  assert.equal(releaseVersionFromTag('runtime', 'v1.2.3'), '')
})

test('new component channels keep their legacy baseline after desktop advances', () => {
  const tags = ['runtime-v0.4.20', 'tui-v0.4.22', 'v0.4.27', 'v0.4.26', 'v0.4.25']
  assert.equal(fallbackReleaseTag('desktop', tags, '0.4.27'), 'v0.4.27')
  assert.equal(fallbackReleaseTag('tui', tags, '0.4.26'), 'tui-v0.4.22')
  assert.equal(fallbackReleaseTag('runtime', tags, '0.4.26'), 'runtime-v0.4.20')
  assert.equal(fallbackReleaseTag('tui', ['v0.4.25', 'v0.4.27', 'v0.4.26'], '0.4.26'), 'v0.4.26')
  assert.equal(fallbackReleaseTag('runtime', ['v0.4.27', 'v0.4.26'], '0.4.26'), 'v0.4.26')
})

test('component staging and notes use the immutable pre-release version as migration baseline', async () => {
  const [release, staging, notes] = await Promise.all([
    readFile('scripts/release.mjs', 'utf8'),
    readFile('scripts/stage-release-version.mjs', 'utf8'),
    readFile('scripts/prepare-release.mjs', 'utf8'),
  ])

  assert.match(release, /fallbackReleaseTag\(component, tags, currentVersion\)/)
  assert.match(staging, /fallbackReleaseTag\(component, tags, currentVersion\)/)
  assert.match(notes, /const baselineVersion = readComponentVersionAtRef\(sourceRef\)/)
  assert.match(notes, /fallbackReleaseTag\([\s\S]*baselineVersion,/)
  assert.match(notes, /componentReleaseSubjects\([\s\S]*selectedComponent/)
  assert.doesNotMatch(notes, /releases\/generate-notes/)
})

test('Tauri reads only the desktop version while component packagers keep their own versions', async () => {
  const [configSource, tauriPackager, tuiPackager, cliStager] = await Promise.all([
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('scripts/package-tauri-release.mjs', 'utf8'),
    readFile('scripts/package-tui.mjs', 'utf8'),
    readFile('scripts/stage-tauri-cli.mjs', 'utf8'),
  ])
  const config = JSON.parse(configSource)

  assert.equal(config.version, 'desktop-package.json')
  assert.match(tauriPackager, /desktop-package\.json/)
  assert.doesNotMatch(tauriPackager, /version from \.\.\/package\.json/)
  assert.match(tuiPackager, /const tuiVersion/)
  assert.match(tuiPackager, /runtimeVersion: packageJson\.version/)
  assert.doesNotMatch(tuiPackager, /sync-tui-version/)
  assert.doesNotMatch(cliStager, /sync-tui-version/)
  await assert.rejects(access('scripts/sync-tui-version.mjs'), (error) => error?.code === 'ENOENT')
})

test('desktop frontend is packaged separately from the runtime payload', async () => {
  const [archiver, seaBuilder, tauriConfigSource, shell, sidecar] = await Promise.all([
    readFile('scripts/archive-component-release.mjs', 'utf8'),
    readFile('scripts/build-sea.mjs', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/lib.rs', 'utf8'),
    readFile('runtime/sidecar.mjs', 'utf8'),
  ])
  const tauriConfig = JSON.parse(tauriConfigSource)

  assert.match(archiver, /async function stageDesktop/)
  assert.match(archiver, /command: 'dist\/index\.html'/)
  assert.match(archiver, /Pisper_Runtime_Node_/)
  assert.match(archiver, /command: 'sidecar-runtime\/runtime\/sidecar\.mjs'/)
  assert.match(archiver, /layout: \['sidecar-runtime\/'\]/)
  assert.doesNotMatch(seaBuilder, /runtimeDir, 'dist'/)
  assert.equal(tauriConfig.bundle.resources['../dist/'], 'desktop/dist/')
  assert.match(shell, /PISPER_FRONTEND_ROOT/)
  assert.match(sidecar, /frontendRoot: process\.env\.PISPER_FRONTEND_ROOT/)
})

test('release workflow builds and finalizes only the selected component channel', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')

  assert.match(workflow, /options:\s*\n\s*- desktop\s*\n\s*- tui\s*\n\s*- runtime/)
  assert.match(workflow, /if: inputs\.component == 'desktop'/)
  assert.match(workflow, /if: inputs\.component == 'tui'/)
  assert.match(workflow, /if: inputs\.component == 'runtime'/)
  assert.match(workflow, /node scripts\/archive-component-release\.mjs desktop/)
  assert.match(workflow, /node scripts\/archive-component-release\.mjs tui/)
  assert.match(workflow, /node scripts\/archive-component-release\.mjs runtime/)
  assert.match(workflow, /node scripts\/validate-component-release-assets\.mjs/)
  assert.match(workflow, /npx tauri signer sign "\$archive"/)
  assert.match(
    workflow,
    /TAURI_SIGNING_PRIVATE_KEY: \$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY \}\}/,
  )
  assert.match(workflow, /--latest="\$MAKE_LATEST"/)
  assert.match(workflow, /MAKE_LATEST: .*inputs\.component == 'desktop'/)
  assert.match(workflow, /desktop\) git add src-tauri\/desktop-package\.json/)
  assert.match(workflow, /tui\) git add src-tui\/Cargo\.toml src-tui\/Cargo\.lock/)
  assert.match(workflow, /runtime\) git add package\.json package-lock\.json/)
  assert.match(workflow, /node scripts\/verify-release-head\.mjs/)
  assert.match(workflow, /git rebase --onto "\$remote_source" "\$RELEASE_SOURCE_SHA" HEAD/)
  assert.match(workflow, /"\$remote_source:refs\/heads\/\$RELEASE_BRANCH"/)
  assert.match(workflow, /cargo test --manifest-path src-tauri\/Cargo\.toml --locked/)
})

test('desktop and TUI launch independently installed signed components with bundled fallback', async () => {
  const [
    updater,
    desktopShell,
    cliManager,
    tuiMain,
    tuiUpdater,
    tuiSidecar,
    bridge,
    permissions,
    settings,
  ] = await Promise.all([
    readFile('crates/component-updater/src/lib.rs', 'utf8'),
    readFile('src-tauri/src/lib.rs', 'utf8'),
    readFile('src-tauri/src/cli_manager.rs', 'utf8'),
    readFile('src-tui/src/main.rs', 'utf8'),
    readFile('src-tui/src/component_update.rs', 'utf8'),
    readFile('src-tui/src/sidecar.rs', 'utf8'),
    readFile('src-tauri/src/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src/features/config/UpdateSettings.tsx', 'utf8'),
  ])

  assert.match(updater, /component signature verification failed/)
  assert.match(updater, /component archive path escapes its root/)
  assert.match(updater, /write_pointer\(&component_root, version\)/)
  assert.match(updater, /Self::Desktop => "v"/)
  assert.match(updater, /Pisper_Desktop/)
  assert.match(updater, /runtime-v/)
  assert.match(updater, /Pisper_Runtime/)
  assert.match(desktopShell, /PISPER_FRONTEND_ROOT/)
  assert.match(desktopShell, /installed_component[\s\S]*Component::Runtime/)
  assert.match(desktopShell, /Installed runtime failed; using bundled runtime/)
  assert.match(cliManager, /preferred_payload/)
  assert.match(cliManager, /preferred_runtime/)
  assert.doesNotMatch(tuiMain, /parse_update_request/)
  assert.doesNotMatch(tuiUpdater, /UpdateSelection|replace_current_tui/)
  assert.match(tuiUpdater, /pub async fn ensure_web/)
  assert.match(tuiUpdater, /pub async fn ensure_runtime/)
  assert.match(tuiSidecar, /deactivate_component/)
  assert.match(bridge, /desktop_check_component_updates/)
  assert.match(bridge, /desktop_install_component_updates/)
  assert.match(permissions, /desktop_restart_for_component_update/)
  assert.match(settings, /appComponents/)
  assert.match(settings, /bundled\.version === info\.version \? bundled : null/)
  assert.match(settings, /bundledCurrent\?\.notes/)
  assert.doesNotMatch(settings, /installComponent\(component\.component\)/)
})

test('release paths select one or every affected component without coupling documentation', () => {
  assert.deepEqual(releaseComponentsForPath('src-tauri/src/lib.rs'), ['desktop'])
  assert.deepEqual(releaseComponentsForPath('src-tui/src/main.rs'), ['tui'])
  assert.deepEqual(releaseComponentsForPath('runtime/index.mjs'), ['runtime'])
  assert.deepEqual(releaseComponentsForPath('src/features/chat/ChatPage.tsx'), ['desktop'])
  assert.deepEqual(releaseComponentsForPath('shared/workflow-graph.mjs'), ['desktop', 'runtime'])
  assert.deepEqual(releaseComponentsForPath('crates/component-updater/src/lib.rs'), [
    'desktop',
    'tui',
  ])
  assert.deepEqual(releaseComponentsForPath('.github/workflows/release.yml'), [])
  assert.deepEqual(releaseComponentsForPath('docs/node-sea-webview.md'), [])
  assert.deepEqual(releaseComponentsForPath('src-tui/README.md'), [])
  assert.deepEqual(releaseComponentsForPath('scripts/release.mjs'), [])
  assert.deepEqual(
    detectReleaseComponents(['src-tauri/src/lib.rs', 'src-tui/src/main.rs', 'runtime/index.mjs']),
    ['desktop', 'tui', 'runtime'],
  )
})

test('component release ranges exclude substantive commits owned by other components', () => {
  const runGit = (args) => {
    const command = args.join(' ')
    if (command === 'diff --name-only --diff-filter=ACMRTUXB v1.0.0..source') {
      return 'src-tauri/src/lib.rs\nruntime/index.mjs\n'
    }
    if (command === 'log --format=%H v1.0.0..source') return 'desktop-commit\nruntime-commit\n'
    if (command.endsWith('-r desktop-commit')) return 'src-tauri/src/lib.rs\n'
    if (command.endsWith('-r runtime-commit')) return 'runtime/index.mjs\n'
    if (command === 'show -s --format=%s desktop-commit') return 'fix(desktop): repair shell\n'
    if (command === 'show -s --format=%s runtime-commit') return 'feat(runtime): add endpoint\n'
    throw new Error(`Unexpected git command: ${command}`)
  }

  assert.deepEqual(componentReleasePaths(runGit, 'desktop', 'v1.0.0', 'source'), [
    'src-tauri/src/lib.rs',
  ])
  assert.deepEqual(componentReleaseSubjects(runGit, 'desktop', 'v1.0.0', 'source'), [
    'fix(desktop): repair shell',
  ])
  assert.deepEqual(componentReleaseSubjects(runGit, 'runtime', 'v1.0.0', 'source'), [
    'feat(runtime): add endpoint',
  ])
})

test('release command auto-detects every changed component and rejects manual scopes', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')

  assert.doesNotMatch(source, /requestedComponent/)
  assert.match(source, /if \(RELEASE_COMPONENTS\[args\[0\]\]\)/)
  assert.match(source, /const candidates = Object\.keys\(RELEASE_COMPONENTS\)/)
  assert.match(source, /if \(paths\.length === 0\) continue/)
  assert.match(
    source,
    /for \(const \[index, \{ component, nextVersion, tag \}\] of plans\.entries\(\)\)/,
  )
  assert.match(source, /componentReleasePaths/)
  assert.match(source, /componentReleaseSubjects/)
  assert.match(source, /`component=\$\{component\}`/)
  assert.match(source, /npmReleaseVersion = resolveVersion\(manifest\.version, input\)/)
  assert.doesNotMatch(source, /const npmBump/)
  assert.match(source, /自动发布组件/)
})

test('desktop release validator requires installers and signed frontend components', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-desktop-assets-'))
  const version = '1.2.3'
  const installers = [
    'latest.json',
    `Pisper_${version}_darwin_aarch64.app.tar.gz`,
    `Pisper_${version}_darwin_aarch64.app.tar.gz.sig`,
    `Pisper_${version}_darwin_aarch64.dmg`,
    `Pisper_${version}_darwin_x86_64.app.tar.gz`,
    `Pisper_${version}_darwin_x86_64.app.tar.gz.sig`,
    `Pisper_${version}_darwin_x86_64.dmg`,
    `Pisper_${version}_linux_x86_64.AppImage`,
    `Pisper_${version}_linux_x86_64.AppImage.sig`,
    `Pisper_${version}_linux_x86_64.deb`,
    `Pisper_${version}_windows_x86_64-setup.exe`,
    `Pisper_${version}_windows_x86_64-setup.exe.sig`,
  ]
  const components = ['darwin_aarch64', 'darwin_x86_64', 'linux_x86_64', 'windows_x86_64']
    .map((platform) => `Pisper_Desktop_${version}_${platform}.tar.gz`)
    .flatMap((archive) => [archive, `${archive}.sig`])

  try {
    await Promise.all(
      [...installers, ...components].map((name) => writeFile(join(directory, name), 'artifact')),
    )
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('runtime release validator requires signed SEA and Node platform archives', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-runtime-assets-'))
  const platforms = ['darwin_aarch64', 'darwin_x86_64', 'linux_x86_64', 'windows_x86_64']
  const archives = platforms.flatMap((platform) => [
    `Pisper_Runtime_1.2.3_${platform}.tar.gz`,
    `Pisper_Runtime_Node_1.2.3_${platform}.tar.gz`,
  ])
  const expected = archives.flatMap((archive) => [archive, `${archive}.sig`])

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'runtime', 'runtime-v1.2.3', directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('component release validator requires signed distributions and thin TUI updates', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-component-assets-'))
  const platforms = ['darwin_aarch64', 'darwin_x86_64', 'linux_x86_64', 'windows_x86_64']
  const archives = platforms.flatMap((platform) => [
    `Pisper_TUI_1.2.3_${platform}.tar.gz`,
    `Pisper_TUI_Component_1.2.3_${platform}.tar.gz`,
  ])
  const expected = archives.flatMap((archive) => [archive, `${archive}.sig`])

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'tui', 'tui-v1.2.3', directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)

    await writeFile(join(directory, 'unexpected.txt'), 'artifact')
    const invalid = spawnSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'tui', 'tui-v1.2.3', directory],
      { encoding: 'utf8' },
    )
    assert.notEqual(invalid.status, 0)
    assert.match(invalid.stderr, /Unexpected release assets: unexpected\.txt/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
