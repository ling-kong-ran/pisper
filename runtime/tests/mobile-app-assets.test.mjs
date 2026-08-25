// 移动端发布资产门禁：图标必须由桌面 ICNS 同步，并在平台工程初始化后覆盖模板图标。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { isAppExclusivePath, isAppOwnedPath } from '../../scripts/app-paths.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'

function pngSize(buffer) {
  assert.equal(buffer.subarray(1, 4).toString('ascii'), 'PNG')
  return [buffer.readUInt32BE(16), buffer.readUInt32BE(20)]
}

test('Android 与 iOS 图标由桌面品牌图标生成', async () => {
  const [script, androidIcon, iosIcon] = await Promise.all([
    readFile('scripts/sync-mobile-icons.mjs', 'utf8'),
    readFile('src-tauri/icons/android/mipmap-xxxhdpi/ic_launcher.png'),
    readFile('src-tauri/icons/ios/AppIcon-512@2x.png'),
  ])
  assert.match(script, /src-tauri["'], ["']icons/)
  assert.match(script, /icon\.icns/)
  assert.match(script, /extractIcnsPng\(readFileSync\(icnsPath\), 'ic10'\)/)
  assert.deepEqual(pngSize(androidIcon), [192, 192])
  assert.deepEqual(pngSize(iosIcon), [1024, 1024])
  assert.equal(isAppOwnedPath('src-tauri/icons/android/mipmap-mdpi/ic_launcher.png'), true)
  assert.equal(isAppOwnedPath('src-tauri/icons/ios/AppIcon-512@2x.png'), true)
})

test('共享产品路径同时归 App 与原组件发布通道', () => {
  const cases = [
    ['src-tauri/Cargo.lock', ['desktop']],
    ['src-tauri/Cargo.toml', ['desktop']],
    ['src-tauri/src/iroh_tunnel.rs', ['desktop']],
    ['src-tauri/src/lib.rs', ['desktop']],
    ['src/app/App.tsx', ['desktop']],
    ['runtime/index.mjs', ['runtime']],
    ['runtime/mobile-embedded.mjs', ['runtime']],
    ['shared/workflow-graph.mjs', ['desktop', 'runtime']],
    ['package-lock.json', ['desktop', 'runtime']],
    ['scripts/sea-runtime.mjs', ['desktop', 'runtime']],
  ]
  for (const [path, components] of cases) {
    assert.equal(isAppOwnedPath(path), true)
    assert.equal(isAppExclusivePath(path), false)
    assert.deepEqual(releaseComponentsForPath(path), components)
  }
  assert.equal(isAppOwnedPath('runtime/tests/mobile-app-assets.test.mjs'), false)
  assert.equal(isAppExclusivePath('src-tauri/src/mobile/proxy.rs'), true)
  assert.deepEqual(releaseComponentsForPath('src-tauri/src/mobile/proxy.rs'), [])
  assert.equal(isAppExclusivePath('src-tauri/mobile/node-host/android/node_host.cpp'), true)
  assert.equal(isAppExclusivePath('src-tauri/mobile/android/MainActivity.kt'), true)
  assert.equal(isAppExclusivePath('src-tauri/mobile-device-plugin/Cargo.toml'), true)
  assert.equal(isAppExclusivePath('src-tauri/Info.ios.plist'), true)
  assert.equal(isAppExclusivePath('public/mobile-startup.html'), true)
  assert.equal(isAppExclusivePath('scripts/stage-mobile-node-android.mjs'), true)
  assert.equal(isAppExclusivePath('scripts/stage-mobile-node-ios.mjs'), true)
  assert.equal(isAppExclusivePath('scripts/mobile-node-ios-smoke-view-controller.m'), true)
  assert.equal(isAppExclusivePath('scripts/smoke-mobile-node-ios.sh'), true)
  assert.equal(isAppExclusivePath('scripts/verify-android-page-size.sh'), true)
  assert.equal(isAppExclusivePath('scripts/verify-tauri-signature.mjs'), true)
})

test('平台初始化流程不会保留 Tauri 模板图标', async () => {
  const [androidSetup, androidBuild, workflow] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile('scripts/build-mobile-android.mjs', 'utf8'),
    readFile('.github/workflows/release-app.yml', 'utf8'),
  ])
  const androidInit = androidSetup.indexOf("'android', 'init'")
  const androidIcons = androidSetup.indexOf("'sync-mobile-icons.mjs'")
  assert.ok(androidInit >= 0 && androidIcons > androidInit)
  assert.match(androidBuild, /process\.platform === 'win32'/)
  assert.match(androidBuild, /kotlin\.incremental/)
  assert.match(androidBuild, /kotlin\.incremental\.useClasspathSnapshot/)
  assert.match(androidBuild, /targetIndex >= 0 \? process\.argv\[targetIndex \+ 1\] : 'aarch64'/)
  assert.match(androidBuild, /build-mobile-runtime\.mjs/)
  assert.match(androidBuild, /pisper-embedded-runtime\.tar\.gz/)
  assert.match(androidBuild, /pisper-embedded-runtime\.tgz/)
  const androidWorkflowInit = workflow.indexOf('run: node scripts/setup-mobile-android.mjs')
  const androidRuntimeAssets = workflow.indexOf(
    'cp embedded-runtime/pisper-embedded-runtime.tar.gz',
  )
  assert.ok(androidWorkflowInit >= 0 && androidRuntimeAssets > androidWorkflowInit)

  const iosIcons = workflow.indexOf('node scripts/sync-mobile-icons.mjs')
  const iosInit = workflow.indexOf('npx tauri ios init')
  assert.ok(iosIcons >= 0 && iosInit > iosIcons)
})

test('Android arm64 native 产物强制兼容 16 KB 内存页', async () => {
  const [setup, cmake, verifier, workflow] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile('src-tauri/mobile/node-host/android/CMakeLists.txt', 'utf8'),
    readFile('scripts/verify-android-page-size.sh', 'utf8'),
    readFile('.github/workflows/release-app.yml', 'utf8'),
  ])

  assert.match(setup, /CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS/)
  assert.match(setup, /max-page-size=16384/)
  assert.match(cmake, /target_link_options\(pisper_node_host/)
  assert.match(cmake, /max-page-size=16384/)
  assert.match(verifier, /lib\/arm64-v8a\/\*\.so/)
  assert.match(verifier, /MIN_ALIGNMENT=\$\(\(16 \* 1024\)\)/)
  assert.match(verifier, /zipalign.*-c -P 16 4|"\$ZIPALIGN" -c -P 16 4/)
  assert.match(workflow, /build-tools;35\.0\.0/)
  assert.match(workflow, /zipalign" -P 16 -f 4/)
  assert.match(workflow, /verify-android-page-size\.sh/)
})

test('移动壳仅在核心 Runtime API 合同通过后挂载业务界面', async () => {
  const [shell, startupPage] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('public/mobile-startup.html', 'utf8'),
  ])

  for (const path of [
    '/api/client-info',
    '/api/runtime/capabilities',
    '/api/config',
    '/api/sessions',
  ]) {
    assert.match(shell, new RegExp(path.replaceAll('/', '\\/')))
  }
  assert.match(shell, /validate_startup_contract_values/)
  assert.match(shell, /redirect\(reqwest::redirect::Policy::none\(\)\)/)
  assert.match(shell, /fetch_startup_cookie/)
  assert.match(shell, /header\(reqwest::header::COOKIE, cookie\)/)
  assert.doesNotMatch(shell, /\.bearer_auth\(token\)/)
  assert.match(shell, /WebviewUrl::App\("mobile-startup\.html"\.into\(\)\)/)
  assert.match(
    shell,
    /WebviewWindowBuilder::new[\s\S]*?\.build\(\)\?;[\s\S]*?start_initial_local_runtime\(/,
  )
  assert.match(shell, /spawn_blocking\(move \|\| runtime\.ensure_started\(\)\)/)
  assert.match(shell, /window\.location\.replace/)
  assert.match(shell, /PageLoadEvent::Finished/)
  assert.match(shell, /call_method\(webview, "clearHistory", "\(\)V"/)
  assert.doesNotMatch(shell, /window\.navigate\(url\)/)
  assert.doesNotMatch(shell, /let on_device_url/)
  assert.doesNotMatch(shell, /WebviewUrl::App\("index\.html"\.into\(\)\)/)
  assert.match(startupPage, /data-phase="starting"/)
  assert.match(startupPage, /@keyframes runtime-progress/)
  assert.match(startupPage, /window\.setTimeout\(loadState, 300\)/)
  assert.match(shell, /mobile_retry_local_startup/)
  assert.match(startupPage, /mobile_retry_local_startup/)
  assert.doesNotMatch(startupPage, /mobile_enter_local/)
  assert.match(startupPage, /mobile_leave_local/)
  assert.match(startupPage, /window\.location\.replace/)
  assert.doesNotMatch(startupPage, /fetch\(['"]\/api\//)
})

test('Android WebView renderer 退出后重建宿主并恢复当前路由', async () => {
  const [setup, activity] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile('src-tauri/mobile/android/MainActivity.kt', 'utf8'),
  ])

  assert.match(setup, /WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION/)
  assert.match(setup, /override fun onRenderProcessGone/)
  assert.match(setup, /MainActivity\.recoverFromRendererCrash/)
  assert.match(setup, /copyFileSync\(mainActivitySourcePath, mainActivityTargetPath\)/)
  assert.match(activity, /\(webView\.parent as\? ViewGroup\)\?\.removeView\(webView\)/)
  assert.match(activity, /webView\.destroy\(\)/)
  assert.match(activity, /recreate\(\)/)
  assert.match(activity, /RENDERER_RECOVERY_URL/)
  assert.match(activity, /webView\.post \{ webView\.loadUrl\(recoveryUrl\) \}/)
  assert.match(activity, /return true/)
})

test('标准移动包只声明联系人、相机与前台定位权限', async () => {
  const [setup, androidPlugin, iosInfo] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('src-tauri/Info.ios.plist', 'utf8'),
  ])
  for (const permission of [
    'android.permission.READ_CONTACTS',
    'android.permission.CAMERA',
    'android.permission.ACCESS_COARSE_LOCATION',
    'android.permission.ACCESS_FINE_LOCATION',
  ]) {
    assert.match(`${setup}\n${androidPlugin}`, new RegExp(permission.replaceAll('.', '\\.')))
  }
  for (const forbidden of [
    'READ_SMS',
    'RECEIVE_SMS',
    'SEND_SMS',
    'CALL_PHONE',
    'QUERY_ALL_PACKAGES',
    'ACCESS_BACKGROUND_LOCATION',
    'BIND_ACCESSIBILITY_SERVICE',
  ]) {
    assert.doesNotMatch(
      `${setup}\n${androidPlugin}`,
      new RegExp(`android\\.permission\\.${forbidden}`),
    )
  }
  assert.match(iosInfo, /NSCameraUsageDescription/)
  assert.match(iosInfo, /NSContactsUsageDescription/)
  assert.match(iosInfo, /NSLocationWhenInUseUsageDescription/)
  assert.doesNotMatch(iosInfo, /NSLocationAlways/)
})

test('外部应用操作只使用用户可见的标准系统入口', async () => {
  const [tool, client, androidPlugin, iosPlugin, store] = await Promise.all([
    readFile('runtime/tools/app/mobile-device.mjs', 'utf8'),
    readFile('src/features/chat/mobile-operations.ts', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('src-tauri/mobile-device-plugin/ios/Sources/MobileDevicePlugin.swift', 'utf8'),
    readFile('src-tauri/src/mobile/store.rs', 'utf8'),
  ])
  for (const operation of [
    'apps.open_url',
    'apps.open_map',
    'apps.open_system_settings',
    'apps.open_dialer',
    'apps.compose_sms',
    'apps.open_app',
  ]) {
    assert.match(`${tool}\n${client}\n${androidPlugin}\n${iosPlugin}`, new RegExp(operation))
  }
  assert.doesNotMatch(store, /DeviceCapabilities|device_capabilities/)
  assert.match(androidPlugin, /Intent\.ACTION_VIEW/)
  assert.match(androidPlugin, /Intent\.ACTION_DIAL/)
  assert.match(androidPlugin, /Intent\.ACTION_SENDTO/)
  assert.match(androidPlugin, /Intent\(Intent\.ACTION_MAIN\)[\s\S]*Intent\.CATEGORY_LAUNCHER/)
  assert.doesNotMatch(androidPlugin, /makeMainSelectorActivity/)
  assert.match(iosPlugin, /UIApplication\.shared\.open/)
  assert.match(tool, /FORBIDDEN_APP_SCHEMES/)
  assert.doesNotMatch(`${androidPlugin}\n${iosPlugin}`, /QUERY_ALL_PACKAGES|SEND_SMS|CALL_PHONE/)
})

test('移动设备操作通过当前会话 SSE 与原生桥闭环', async () => {
  const [routes, runtime, dispatcher, client, native, permissions] = await Promise.all([
    readFile('runtime/http/routes/sessions-runtime.mjs', 'utf8'),
    readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/features/chat/mobile-operations.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
  ])
  assert.match(routes, /mobile-operations\/:operationId/)
  assert.match(routes, /mobileClient: isMobileAppRequest/)
  assert.match(runtime, /this\.mobileOperations\.attach\(session\.sessionId, emit\)/)
  assert.match(dispatcher, /mobile_operation_request/)
  assert.match(client, /mobile_execute_device_operation/)
  assert.doesNotMatch(native, /device_capabilities|set_device_capability/)
  assert.match(native, /permission_states\(\)[\s\S]*request_permission\(capability\)/)
  assert.match(permissions, /mobile_execute_device_operation/)
})

test('移动 Node 供应链固定来源并在两个平台执行完整性门禁', async () => {
  const [
    metadataText,
    androidStage,
    iosStage,
    iosBuild,
    iosSmoke,
    iosSmokeController,
    workflow,
    setup,
  ] = await Promise.all([
    readFile('scripts/mobile-node-artifacts.json', 'utf8'),
    readFile('scripts/stage-mobile-node-android.mjs', 'utf8'),
    readFile('scripts/stage-mobile-node-ios.mjs', 'utf8'),
    readFile('scripts/build-mobile-node-ios.sh', 'utf8'),
    readFile('scripts/smoke-mobile-node-ios.sh', 'utf8'),
    readFile('scripts/mobile-node-ios-smoke-view-controller.m', 'utf8'),
    readFile('.github/workflows/release-app.yml', 'utf8'),
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
  ])
  const metadata = JSON.parse(metadataText)
  assert.equal(metadata.source.commit, '8a995e179bb2c224029a560ae9c4f9460631b94d')
  assert.equal(metadata.runtime.nodeVersion, '24.18.1')
  assert.equal(metadata.runtime.modulesAbi, 137)
  assert.match(metadata.android.archiveSha256, /^[a-f0-9]{64}$/)
  assert.match(metadata.android.libnodeSha256, /^[a-f0-9]{64}$/)
  assert.match(metadata.ios.archiveSha256, /^[a-f0-9]{64}$/)
  assert.match(metadata.ios.sigstoreBundleSha256, /^[a-f0-9]{64}$/)
  assert.match(androidStage, /['"]verify-blob['"]/)
  assert.match(androidStage, /sha256/)
  assert.match(iosStage, /['"]verify-blob['"]/)
  assert.match(iosStage, /verifyInternalDigests/)
  assert.match(iosStage, /sourceCommit/)
  assert.match(iosBuild, /checkout -q --detach/)
  assert.match(iosBuild, /SOURCE_COMMIT/)
  assert.match(iosBuild, /HEAD\^\{tree\}/)
  assert.match(iosBuild, /MATERIALIZED_TREE/)
  assert.match(iosBuild, /--materialize-only/)
  assert.match(iosBuild, /stage-mobile-node-ios\.mjs/)
  assert.match(
    workflow,
    /stage-mobile-node-android\.mjs release\/mobile-node-android --require-sigstore/,
  )
  assert.match(workflow, /bash scripts\/build-mobile-node-ios\.sh --materialize-only/)
  assert.match(workflow, /stage-mobile-node-ios\.mjs release\/mobile-node-ios --require-sigstore/)
  assert.doesNotMatch(workflow, /npm run mobile:node:ios/)
  assert.match(workflow, /shasum -a 256 -c SHA256SUMS/)
  assert.match(workflow, /lib\/arm64-v8a\/libnode\.so/)
  assert.match(workflow, /NodeMobile\.xcframework/)
  assert.equal(
    (workflow.match(/"resources":\["\.\.\/release\/pisper-embedded-runtime\.tar\.gz"\]/g) || [])
      .length,
    2,
  )
  assert.doesNotMatch(workflow, /"resources":\{"\.\.\/release\/pisper-embedded-runtime\.tar\.gz"/)
  assert.match(workflow, /bash scripts\/smoke-mobile-node-ios\.sh/)
  assert.match(workflow, /Payload\/\[\^\/\]\+\\\.app\/Frameworks\/NodeMobile/)
  assert.match(workflow, /pisper-embedded-runtime\\\.tar\\\.gz/)
  assert.match(iosSmoke, /mobile-node-ios-smoke-view-controller\.m/)
  assert.match(iosSmoke, /--smoke-ui/)
  assert.match(iosSmoke, /stdout-\$TOKEN/)
  assert.match(iosSmoke, /startEmbeddedRuntime/)
  assert.match(iosSmoke, /\/api\/health/)
  assert.match(iosSmoke, /PISPER_IOS_RUNTIME_SMOKE_OK/)
  assert.match(iosSmokeController, /dispatch_async/)
  assert.match(iosSmokeController, /NODE_MOBILE_RUN_TOKEN/)
  assert.match(setup, /pisper-node-host/)
  assert.match(setup, /EmbeddedNodeHost\.kt/)
  assert.match(setup, /libc\+\+_shared\.so/)
  assert.match(setup, /externalNativeBuild/)
  assert.match(
    workflow,
    /Prepare Android project with embedded Node[\s\S]*NDK_HOME:.*27\.3\.13750724/,
  )
  assert.match(workflow, /lib\/arm64-v8a\/libc\+\+_shared\.so/)
})

test('移动端明确区分远程档案与当前 Runtime 路由', async () => {
  await Promise.all([
    assert.rejects(readFile('public/connect.html'), /ENOENT/),
    assert.rejects(readFile('public/connect.js'), /ENOENT/),
  ])
  const [native, permissions, appPaths, settings, pairing] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('scripts/app-paths.mjs', 'utf8'),
    readFile('src/features/config/MobileServerSettings.tsx', 'utf8'),
    readFile('src/features/config/MobilePairingDialog.tsx', 'utf8'),
  ])
  assert.match(native, /mode: Option<String>/)
  assert.match(native, /store\.last_mode\(\)\.map\(str::to_string\)/)
  assert.match(native, /mode\.as_deref\(\) == Some\("remote"\)/)
  assert.match(native, /store\.set_last_mode\("remote"\)/)
  assert.match(native, /last_mode\.as_deref\(\) == Some\("remote"\) && paired/)
  assert.match(native, /set_last_mode\("local"\)/)
  assert.doesNotMatch(native, /connect\.html/)
  assert.doesNotMatch(permissions, /mobile_connect_url/)
  assert.doesNotMatch(appPaths, /public\/connect\.(?:html|js)/)
  assert.match(settings, /state\?\.mode === 'local'/)
  assert.match(settings, /state\.mode === 'remote' && server\.id === state\.activeId/)
  assert.match(settings, /window\.location\.replace\(updated\.proxyUrl\)/)
  assert.match(settings, /<MobilePairingDialog/)
  assert.match(pairing, /plugin:barcode-scanner/)
  assert.match(pairing, /mobile_pair_manual/)
  assert.match(pairing, /window\.location\.replace\(state\.proxyUrl\)/)
})

test('root Android Runtime 构建仅在系统包安装期间绑定构建机设备', async () => {
  const script = await readFile('scripts/build-android-root-runtime.sh', 'utf8')
  const bind = script.indexOf('sudo mount --bind /dev "$ROOTFS/dev"')
  const update = script.indexOf('apt-get update')
  const install = script.indexOf('apt-get install')
  const unmount = script.indexOf('sudo umount "$ROOTFS/dev"', install)
  const archive = script.indexOf('sudo tar --numeric-owner --xattrs --acls')

  assert.match(script, /trap cleanup EXIT/)
  assert.ok(bind >= 0 && bind < update)
  assert.ok(update < install && install < unmount)
  assert.ok(unmount < archive)
})

test('移动 Runtime 本地 staging 使用 App 版本并兼容 Windows Node 24', async () => {
  const [builder, staging] = await Promise.all([
    readFile('scripts/build-mobile-runtime.mjs', 'utf8'),
    readFile('scripts/stage-runtime-closure.mjs', 'utf8'),
  ])
  assert.match(builder, /src-tauri["'], ["']mobile-package\.json/)
  assert.match(builder, /PISPER_APP_VERSION \|\| mobilePackage\.version/)
  assert.match(staging, /dirname\(process\.execPath\)/)
  assert.match(staging, /npm-cli\.js/)
  assert.doesNotMatch(staging, /run\(["']npm\.cmd["']/)
})

test('embedded Node 使用后台线程、真实初始化 READY 与 App 生命周期', async () => {
  const [entry, kotlin, cpp, rustHost, carrier, rootHost] = await Promise.all([
    readFile('runtime/mobile-embedded.mjs', 'utf8'),
    readFile('src-tauri/mobile/node-host/android/EmbeddedNodeHost.kt', 'utf8'),
    readFile('src-tauri/mobile/node-host/android/node_host.cpp', 'utf8'),
    readFile('src-tauri/src/mobile/embedded_runtime.rs', 'utf8'),
    readFile('src-tauri/src/mobile/on_device_runtime.rs', 'utf8'),
    readFile('src-tauri/src/mobile/root_runtime.rs', 'utf8'),
  ])
  assert.match(entry, /export async function startEmbeddedRuntime/)
  assert.match(entry, /PISPER_MOBILE_AUTOSTART === '1'/)
  assert.match(entry, /await pisper\.initialized/)
  assert.ok(entry.indexOf('await pisper.initialized') < entry.indexOf('await report(readyFile, {'))
  assert.doesNotMatch(entry, /process\.exit\s*\(/)
  assert.match(kotlin, /thread\(/)
  assert.match(kotlin, /isDaemon = true/)
  assert.match(cpp, /node::Start/)
  assert.match(rustHost, /PISPER_MOBILE_READY_FILE/)
  assert.match(rustHost, /token: Mutex<Option<String>>/)
  assert.match(rustHost, /started\.load\(Ordering::Acquire\)[\s\S]*wait_until_ready\(&token\)/)
  assert.match(rustHost, /set_var\("PISPER_MOBILE_AUTOSTART", "1"\)/)
  assert.match(rustHost, /runtime_profile != "mobile-embedded"/)
  assert.match(rustHost, /Frameworks\/NodeMobile\.framework\/NodeMobile/)
  assert.match(rustHost, /dlopen/)
  assert.match(rustHost, /dlsym/)
  assert.doesNotMatch(rustHost, /fn node_start\(/)
  assert.match(rustHost, /getClassLoader/)
  assert.match(rustHost, /"loadClass"/)
  assert.doesNotMatch(rustHost, /call_static_method\(\s*"com\/lingkongran\/pisper/)
  assert.match(carrier, /Carrier::Root[\s\S]*Carrier::Embedded/)
  assert.match(carrier, /data_root\.clone\(\)/)
  assert.match(carrier, /lifecycle: Mutex/)
  assert.match(carrier, /远程模式不终止同进程 embedded Node/)
  assert.match(rootHost, /mount --bind \{shared_data\} \{data\}/)
  assert.match(rootHost, /mount --bind \{shared_workspace\} \{workspace\}/)
  assert.match(rootHost, /避免 Runtime 后续日志写入触发 EPIPE/)
})

test('App 发布必须签名并校验共享 Runtime 与平台产物', async () => {
  const workflow = await readFile('.github/workflows/release-app.yml', 'utf8')
  for (const asset of [
    'pisper-embedded-runtime.tar.gz',
    'pisper-root-runtime-android-arm64.tar.gz',
    'pisper-node-mobile-ios.tar.gz',
    'app-universal-release-signed.apk',
    'pisper-ios-unsigned.ipa',
  ]) {
    assert.match(workflow, new RegExp(`${asset.replaceAll('.', '\\.')}\\.minisig`))
  }
  assert.match(workflow, /pisper-embedded-runtime\.tgz/)
  assert.match(workflow, /pisper-root-runtime-android-arm64\.tgz/)
  assert.match(workflow, /needs\['embedded-runtime'\]\.result == 'success'/)
  assert.match(workflow, /needs\['root-runtime'\]\.result == 'success'/)
  assert.equal(workflow.match(/node scripts\/verify-tauri-signature\.mjs/g)?.length, 4)
  assert.doesNotMatch(workflow, /minisign -Vm/)
})
