import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

test('main desktop source and dependencies are Tauri-only', async () => {
  await assert.rejects(access('electron'), (error) => error?.code === 'ENOENT')
  await assert.rejects(access('design'), (error) => error?.code === 'ENOENT')
  await assert.rejects(access('scripts/generate-icons.mjs'), (error) => error?.code === 'ENOENT')
  const [packageSource, seaBuild] = await Promise.all([
    readFile('package.json', 'utf8'),
    readFile('scripts/build-sea.mjs', 'utf8'),
  ])
  const packageJson = JSON.parse(packageSource)
  const dependencies = { ...packageJson.dependencies, ...packageJson.devDependencies }
  for (const name of [
    'electron',
    'electron-builder',
    'electron-updater',
    '@electron/asar',
    'sharp',
  ]) {
    assert.equal(dependencies[name], undefined)
  }
  assert.equal(packageJson.scripts.icons, undefined)
  assert.doesNotMatch(seaBuild, /generate-icons|design[/\\]|join\(root, 'build'\)/)
})

test('transparent desktop pet enables the required macOS Tauri API', async () => {
  const [cargo, config, desktopPet] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/desktop_pet.rs', 'utf8'),
  ])

  assert.match(desktopPet, /\.transparent\(true\)/)
  assert.match(cargo, /features = \["tray-icon", "macos-private-api"\]/)
  assert.equal(JSON.parse(config).app.macOSPrivateApi, true)
})

test('release quality stages both desktop sidecars before checking Rust', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const qualityJob = workflow.slice(workflow.indexOf('quality:'), workflow.indexOf('  build:'))
  const cargoCheck = qualityJob.indexOf('cargo check')

  assert.ok(qualityJob.indexOf('npm run sidecar:sea') < cargoCheck)
  assert.ok(qualityJob.indexOf('npm run tui:stage') < cargoCheck)
})

test('Windows GNU packages carry the WebView2 loader through an explicit Rust target', async () => {
  const [packager, staging] = await Promise.all([
    readFile('scripts/package-tauri-release.mjs', 'utf8'),
    readFile('scripts/stage-tauri-artifacts.mjs', 'utf8'),
  ])

  assert.match(packager, /capture\(process\.env\.RUSTC \|\| 'rustc', \['-vV'\]/)
  assert.match(packager, /rustTarget\.endsWith\('-windows-gnu'\)/)
  assert.match(packager, /tauriTargetArgs\.push\('--target', rustTarget\)/)
  assert.match(packager, /env\.PISPER_TAURI_BUNDLE_DIR = bundleDir/)
  assert.match(staging, /process\.env\.PISPER_TAURI_BUNDLE_DIR/)
})

test('desktop startup refreshes only an existing managed TUI installation', async () => {
  const [manager, desktop] = await Promise.all([
    readFile('src-tauri/src/cli_manager.rs', 'utf8'),
    readFile('src-tauri/src/lib.rs', 'utf8'),
  ])

  assert.match(manager, /pub fn refresh_managed_cli\(app: &AppHandle\)/)
  assert.equal(
    manager.match(
      /if !has_managed_marker\((?:marker|contents)\.as_deref\(\)\) \{\s*return Ok\(false\);/g,
    )?.length,
    2,
  )
  assert.match(manager, /marker\.as_deref\(\) != Some\(expected_marker\(app\)\?\.as_str\(\)\)/)
  assert.match(manager, /payload_size=\{payload_size\}/)
  assert.match(manager, /install_windows\(app\)\?/)
  assert.match(manager, /install_unix\(app\)\?/)
  const refresh = desktop.indexOf('cli_manager::refresh_managed_cli(app.handle())')
  const sidecar = desktop.indexOf('let (child, ready) = start_sidecar(app)?')
  assert.ok(refresh >= 0)
  assert.ok(sidecar > refresh)
  assert.match(desktop, /if let Err\(error\) = cli_manager::refresh_managed_cli\(app\.handle\(\)\)/)
})

test('desktop bundles the TUI behind the narrow CLI management bridge', async () => {
  const [configSource, packageSource, bridge, permissions, manager] = await Promise.all([
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('package.json', 'utf8'),
    readFile('src-tauri/src/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src-tauri/src/cli_manager.rs', 'utf8'),
  ])
  const config = JSON.parse(configSource)
  const packageJson = JSON.parse(packageSource)

  assert.deepEqual(config.bundle.externalBin, [
    'binaries/pisper-sidecar',
    'binaries/pisper-cli',
    'binaries/agent-sandboxd',
  ])
  assert.match(packageJson.scripts['desktop:webview:build'], /npm run sandbox:stage/)
  assert.match(packageJson.scripts['desktop:webview:build'], /npm run tui:stage/)
  for (const command of [
    'desktop_get_cli_status',
    'desktop_install_cli',
    'desktop_uninstall_cli',
  ]) {
    assert.match(bridge, new RegExp(command))
    assert.match(permissions, new RegExp(command))
  }
  assert.match(manager, /PISPER_CLI_MANAGED_V1/)
  assert.match(manager, /WM_SETTINGCHANGE/)
  assert.match(manager, /\.local.*bin/s)
})

test('release workflow validates the exact Tauri asset set before upload', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const validator = workflow.indexOf('node scripts/validate-tauri-release-assets.mjs')
  const upload = workflow.indexOf('softprops/action-gh-release')

  assert.ok(validator >= 0)
  assert.ok(upload > validator)
  assert.doesNotMatch(workflow, /gh release download/)
})

test('release assets reject legacy updater metadata and unexpected files', async () => {
  const version = '0.4.1'
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-assets-'))
  const expected = [
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

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)

    await writeFile(join(directory, 'latest.yml'), 'version: 0.3.3')
    const invalid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.notEqual(invalid.status, 0)
    assert.match(invalid.stderr, /Unexpected release assets: latest\.yml/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
