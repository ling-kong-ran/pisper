import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
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

test('SEA builder imports and applies the final size-manifest assertion', async () => {
  const seaBuild = await readFile('scripts/build-sea.mjs', 'utf8')

  assert.match(
    seaBuild,
    /import\s*\{[^}]*\bassertSizeManifest\b[^}]*\}\s*from '\.\/sea-runtime\.mjs'/,
  )
  assert.match(seaBuild, /assertSizeManifest\(manifest, \{ requireExecutable: true \}\)/)
})

test('new desktop shells use component updates while legacy clients retain release metadata', async () => {
  const [cargo, config, library, bridge, permissions, workflow, updateHook] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('.github/workflows/release.yml', 'utf8'),
    readFile('src/features/updates/useAppUpdate.ts', 'utf8'),
  ])

  assert.doesNotMatch(cargo, /tauri-plugin-updater/)
  assert.equal(JSON.parse(config).plugins?.updater, undefined)
  assert.doesNotMatch(library, /desktop_(?:check_for|download|install)_update/)
  assert.doesNotMatch(bridge, /checkForUpdates|downloadUpdate|installUpdate/)
  assert.doesNotMatch(permissions, /desktop_(?:check_for|download|install)_update/)
  assert.match(workflow, /Create multi-platform updater manifest/)
  assert.match(workflow, /create-tauri-update-manifest\.mjs/)
  assert.match(updateHook, /if \(bridge\?\.checkForUpdates\)/)
  assert.match(updateHook, /legacyShellUpdateRef\.current/)
})

test('main desktop window leaves HTML5 drag and drop to the webview', async () => {
  const library = await readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8')
  const mainWindow = library.slice(
    library.indexOf('WebviewWindowBuilder::new(app, "main"'),
    library.indexOf('fn show_main_window'),
  )

  assert.match(mainWindow, /\.disable_drag_drop_handler\(\)/)
})

test('closing the macOS main window keeps it reopenable from the Dock', async () => {
  const library = await readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8')

  assert.match(
    library,
    /WindowEvent::CloseRequested[\s\S]*api\.prevent_close\(\)[\s\S]*window\.hide\(\)/,
  )
  assert.match(library, /RunEvent::Reopen \{ \.\. \}[\s\S]*show_main_window\(app\)/)
})

test('transparent desktop pet enables the required macOS Tauri API', async () => {
  const [cargo, config, desktopPet] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/desktop_shell/desktop_pet.rs', 'utf8'),
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
  assert.doesNotMatch(qualityJob, /sandbox:stage|agent-sandboxd/)
})

test('desktop updater metadata reads bundled component versions from the desktop manifest', async () => {
  const build = await readFile('src-tauri/build.rs', 'utf8')

  assert.match(build, /fn desktop_package\(\) -> serde_json::Value/)
  assert.match(build, /read_to_string\("desktop-package\.json"\)/)
  assert.match(build, /manifest_version\(&desktop_package, &\["version"\]\)/)
  assert.match(build, /manifest_version\(&desktop_package, &\["bundled", "runtime"\]\)/)
  assert.match(build, /manifest_version\(&desktop_package, &\["bundled", "tui"\]\)/)
  assert.doesNotMatch(build, /package_version\(/)
  assert.doesNotMatch(build, /\.\.\/package\.json|\.\.\/src-tui\/Cargo\.toml/)
})

test('Windows test harnesses reuse the valid Tauri resource without replacing binary input', async () => {
  const [build, library, binary] = await Promise.all([
    readFile('src-tauri/build.rs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
    readFile('src-tauri/src/main.rs', 'utf8'),
  ])

  assert.match(build, /std::fs::copy\(&generated, &test_resource\)/)
  assert.match(build, /cargo:rustc-link-search=native=/)
  assert.doesNotMatch(build, /std::fs::rename|!<arch>|rustc-link-arg-tests/)
  assert.match(library, /#\[cfg\(all\(test, target_os = "windows"\)\)\]/)
  assert.match(library, /#\[link\(name = "pisper_test_resource", kind = "static"\)\]/)
  assert.doesNotMatch(binary, /pisper_test_resource/)
})

test('desktop packaging rebuilds and audits production assets before invoking Tauri', async () => {
  const packager = await readFile('scripts/package-tauri-release.mjs', 'utf8')
  const frontendBuild = packager.indexOf("path.join(root, 'scripts', 'build-frontend.mjs')")
  const bundleAudit = packager.indexOf("path.join(root, 'scripts', 'check-bundle-budget.mjs')")
  const tauriBuild = packager.indexOf("const buildArgs = [tauriCli, 'build'")

  assert.ok(frontendBuild >= 0)
  assert.ok(bundleAudit > frontendBuild)
  assert.ok(tauriBuild > bundleAudit)
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

test('Windows standard and offline installers preserve distinct WebView2 strategies when signed', async () => {
  const config = JSON.parse(await readFile('src-tauri/tauri.conf.json', 'utf8'))
  const updaterConfig = JSON.parse(await readFile('src-tauri/tauri.updater.conf.json', 'utf8'))
  const offlineConfig = JSON.parse(
    await readFile('src-tauri/tauri.windows-offline.conf.json', 'utf8'),
  )
  // 签名不能改变依赖策略；完整运行时仅进入单独的离线安装包。
  for (const windows of [
    config.bundle.windows,
    { ...config.bundle.windows, ...updaterConfig.bundle?.windows },
  ]) {
    assert.equal(windows.webviewInstallMode.type, 'embedBootstrapper')
    assert.equal(windows.webviewInstallMode.silent, true)
    // Tauri 的最低版本升级分支调用在线 Edge Update，不应重新引入安装时联网。
    assert.ok(!windows.minimumWebview2Version)
  }
  const offline = {
    ...config.bundle.windows,
    ...updaterConfig.bundle?.windows,
    ...offlineConfig.bundle.windows,
  }
  assert.equal(offline.webviewInstallMode.type, 'offlineInstaller')
  assert.equal(offline.webviewInstallMode.silent, true)
  assert.ok(!offline.minimumWebview2Version)
})

test('Windows dependency notices precede downloads and provide localized recovery instructions', async () => {
  const config = JSON.parse(await readFile('src-tauri/tauri.conf.json', 'utf8'))
  const nsis = config.bundle.windows.nsis
  const hooks = await readFile(join('src-tauri', nsis.installerHooks), 'utf8')
  assert.match(hooks, /Section "-Pisper prerequisites"/)
  assert.match(hooks, /ReadRegStr.*HKLM/)
  assert.match(hooks, /ReadRegStr.*HKCU/)
  assert.match(hooks, /IfSilent/)
  assert.match(hooks, /MessageBox MB_OKCANCEL/)
  assert.match(hooks, /PISPER_OFFLINE_INSTALLER/)
  assert.match(hooks, /offline-setup\.exe/)
  const requiredKeys = [
    'installingWebview2',
    'webview2Downloading',
    'webview2AbortError',
    'webview2InstallError',
  ]
  for (const language of ['English', 'SimpChinese']) {
    assert.ok(nsis.languages.includes(language))
    const strings = await readFile(join('src-tauri', nsis.customLanguageFiles[language]), 'utf8')
    for (const key of requiredKeys) assert.match(strings, new RegExp(`LangString ${key} `))
    assert.match(strings, /offline-setup\.exe/)
    assert.match(strings, /exit code \$1|错误码 \$1|错误.*\$1/)
    assert.doesNotMatch(strings, /\{\{product_name\}\}/)
  }
  const desktop = await readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8')
  assert.ok(
    desktop.indexOf('tauri::webview_version()') < desktop.indexOf('tauri::Builder::default()'),
  )
  assert.match(desktop, /missing_bundled_file/)
  assert.match(desktop, /StartupStage::Runtime/)
  assert.match(desktop, /StartupStage::Desktop/)
  assert.doesNotMatch(desktop, /expect\("failed to build Pisper WebView application"\)/)
})

test('desktop startup refreshes only an existing managed TUI installation', async () => {
  const [manager, desktop] = await Promise.all([
    readFile('src-tauri/src/desktop_shell/cli_manager.rs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
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
    readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src-tauri/src/desktop_shell/cli_manager.rs', 'utf8'),
  ])
  const config = JSON.parse(configSource)
  const packageJson = JSON.parse(packageSource)

  assert.deepEqual(config.bundle.externalBin, ['binaries/pisper-sidecar', 'binaries/pisper-cli'])
  assert.doesNotMatch(packageJson.scripts['desktop:webview:build'], /sandbox/)
  assert.match(packageJson.scripts['desktop:webview:build'], /npm run tui:stage/)
  assert.doesNotMatch(manager, /PISPER_SANDBOX_PATH|agent-sandboxd/)
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

test('release workflow validates all assets before the bot commits, tags, and publishes', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const validator = workflow.indexOf('node scripts/validate-tauri-release-assets.mjs')
  const versionCommit = workflow.indexOf(
    'git commit -m "chore(release-$RELEASE_COMPONENT): $RELEASE_TAG"',
  )
  const atomicPush = workflow.indexOf('git push --atomic origin')
  const draftUpload = workflow.indexOf('gh release create "$RELEASE_TAG"')
  const publish = workflow.indexOf('gh release edit "$RELEASE_TAG"')

  assert.ok(validator >= 0)
  assert.ok(versionCommit > validator)
  assert.ok(atomicPush > versionCommit)
  assert.ok(draftUpload > atomicPush)
  assert.ok(publish > draftUpload)
  assert.match(workflow, /git config user\.name "github-actions\[bot\]"/)
  assert.match(workflow, /--verify-tag/)
  assert.match(workflow, /--draft/)
  assert.match(workflow, /trap rollback ERR/)
  assert.doesNotMatch(workflow, /push:\s*\n\s*tags:/)
  assert.doesNotMatch(workflow, /softprops\/action-gh-release|gh release download/)
})

test('release workflow stages version metadata without exposing it before all builds pass', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const prepare = workflow.slice(workflow.indexOf('  prepare:'), workflow.indexOf('  quality:'))
  const release = workflow.slice(workflow.indexOf('  release:'))

  assert.match(workflow, /workflow_dispatch:/)
  assert.match(workflow, /component:/)
  assert.match(workflow, /source_sha:/)
  assert.match(prepare, /git fetch origin release/)
  assert.match(
    prepare,
    /node scripts\/verify-release-head\.mjs "\$RELEASE_SOURCE_SHA" origin\/release/,
  )
  assert.match(
    prepare,
    /node scripts\/stage-release-version\.mjs "\$RELEASE_COMPONENT" "\$RELEASE_VERSION"/,
  )
  assert.match(prepare, /name: release-source/)
  assert.match(prepare, /src-tauri\/desktop-package\.json/)
  assert.match(release, /remote_source=.*origin\/\$RELEASE_BRANCH/)
  assert.match(
    release,
    /node scripts\/verify-release-head\.mjs "\$RELEASE_SOURCE_SHA" "\$remote_source"/,
  )
  assert.match(release, /git rebase --onto "\$remote_source" "\$RELEASE_SOURCE_SHA" HEAD/)
  assert.match(release, /refs\/tags\/\$RELEASE_TAG/)
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
    `Pisper_${version}_windows_x86_64-offline-setup.exe`,
    `Pisper_${version}_windows_x86_64-offline-setup.exe.sig`,
    ...['darwin_aarch64', 'darwin_x86_64', 'linux_x86_64', 'windows_x86_64']
      .map((platform) => `Pisper_Desktop_${version}_${platform}.tar.gz`)
      .flatMap((archive) => [archive, `${archive}.sig`]),
  ]

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)

    for (const name of expected.filter((entry) => entry.includes('-offline-setup.exe'))) {
      await rm(join(directory, name))
      const missingOffline = spawnSync(
        process.execPath,
        ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
        { encoding: 'utf8' },
      )
      assert.notEqual(missingOffline.status, 0)
      assert.match(missingOffline.stderr, /Missing release assets:.*offline-setup/)
      await writeFile(join(directory, name), 'artifact')
    }

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

test('Windows variants stage independently and only the standard installer enters automatic updates', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-windows-variants-'))
  const bundle = join(directory, 'bundle')
  const staging = join(directory, 'staged')
  const { version } = JSON.parse(await readFile('src-tauri/desktop-package.json', 'utf8'))
  const installerName = `Pisper_${version}_windows_x86_64-setup.exe`
  const offlineName = `Pisper_${version}_windows_x86_64-offline-setup.exe`
  const output = join(directory, 'latest.json')
  const stage = (offline = false) =>
    spawnSync(
      process.execPath,
      [
        '--input-type=module',
        '-e',
        // 仅模拟脚本的平台分支；真实 NSIS 构建与签名由 Windows CI 验证。
        `Object.defineProperty(process, 'platform', { value: 'win32' });
     Object.defineProperty(process, 'arch', { value: 'x64' });
     process.argv.push('--require-signature'${offline ? ", '--windows-offline'" : ''});
     await import('./scripts/stage-tauri-artifacts.mjs');`,
      ],
      {
        encoding: 'utf8',
        env: { ...process.env, PISPER_TAURI_BUNDLE_DIR: bundle, PISPER_TAURI_STAGE_DIR: staging },
      },
    )
  try {
    await mkdir(bundle)
    await writeFile(join(bundle, 'Pisper-test-setup.exe'), 'standard-installer')
    await writeFile(join(bundle, 'Pisper-test-setup.exe.sig'), 'standard-signature')
    const standard = stage()
    assert.equal(standard.status, 0, standard.stderr)
    await writeFile(join(bundle, 'Pisper-test-setup.exe'), 'offline-installer')
    await writeFile(join(bundle, 'Pisper-test-setup.exe.sig'), 'offline-signature')
    const offline = stage(true)
    assert.equal(offline.status, 0, offline.stderr)
    assert.equal(
      await readFile(join(staging, 'windows-x86_64', installerName), 'utf8'),
      'standard-installer',
    )
    assert.equal(
      await readFile(join(staging, 'windows-x86_64-offline', offlineName), 'utf8'),
      'offline-installer',
    )
    const manifest = spawnSync(
      process.execPath,
      ['scripts/create-tauri-update-manifest.mjs', `v${version}`, staging, '--output', output],
      { encoding: 'utf8' },
    )
    assert.equal(manifest.status, 0, manifest.stderr)
    const { platforms } = JSON.parse(await readFile(output, 'utf8'))
    assert.equal(platforms['windows-x86_64'].signature, 'standard-signature')
    assert.ok(platforms['windows-x86_64'].url.endsWith(`/${installerName}`))
    assert.equal(Object.keys(platforms).length, 1)
    await rm(join(staging, 'windows-x86_64'), { recursive: true })
    const offlineOnly = spawnSync(
      process.execPath,
      ['scripts/create-tauri-update-manifest.mjs', `v${version}`, staging, '--output', output],
      { encoding: 'utf8' },
    )
    assert.notEqual(offlineOnly.status, 0)
    assert.match(offlineOnly.stderr, /No signed Tauri updater artifacts/)
    await rm(join(bundle, 'Pisper-test-setup.exe.sig'))
    assert.match(stage(true).stderr, /Missing updater signature/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
