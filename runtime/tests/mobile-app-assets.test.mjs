// 移动端发布资产门禁：图标必须由桌面 ICNS 同步，并在平台工程初始化后覆盖模板图标。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { isAppExclusivePath, isAppOwnedPath } from '../../scripts/app-paths.mjs'
import { injectIosPrivacyManifest } from '../../scripts/setup-mobile-ios.mjs'
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
    ['crates/tauri-plugin-dns-sd/src/lib.rs', ['desktop']],
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
  assert.equal(isAppExclusivePath('src-tauri/tauri.mobile-ios.conf.json'), true)
  assert.equal(isAppExclusivePath('.github/workflows/build-store-app.yml'), true)
  assert.equal(isAppExclusivePath('public/mobile-startup.html'), true)
  assert.equal(isAppExclusivePath('scripts/stage-mobile-node-android.mjs'), true)
  assert.equal(isAppExclusivePath('scripts/stage-mobile-node-ios.mjs'), true)
  assert.equal(isAppExclusivePath('scripts/setup-mobile-ios.mjs'), true)
  assert.equal(isAppExclusivePath('scripts/mobile-node-ios-smoke-view-controller.m'), true)
  assert.equal(isAppExclusivePath('scripts/smoke-mobile-node-ios.sh'), true)
  assert.equal(isAppExclusivePath('scripts/test-ios-dns-sd.sh'), true)
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
  const [
    shell,
    proxy,
    startupPage,
    permissions,
    staticHandler,
    recovery,
    http,
    chat,
    buildScript,
    iosBuild,
  ] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/src/mobile/proxy.rs', 'utf8'),
    readFile('public/mobile-startup.html', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('runtime/http/static-handler.mjs', 'utf8'),
    readFile('src/lib/mobile-runtime-recovery.ts', 'utf8'),
    readFile('src/lib/http.ts', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
    readFile('src-tauri/build.rs', 'utf8'),
    readFile('scripts/build-mobile-ios.mjs', 'utf8'),
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
  assert.match(shell, /window\s*\.navigate\(recovery_navigation_url/)
  assert.doesNotMatch(shell, /replace_with_authenticated_runtime/)
  assert.doesNotMatch(shell, /WebviewUrl::External/)
  assert.doesNotMatch(shell, /let on_device_url/)
  assert.doesNotMatch(shell, /WebviewUrl::App\("index\.html"\.into\(\)\)/)
  assert.match(shell, /proxy\.configure_local_runtime\(&status\.url\)/)
  assert.match(proxy, /path\.starts_with\("\/api\/"\) && remote_mode/)
  assert.match(
    proxy,
    /forward_remote\(proxy, request\)\.await[\s\S]*forward_local\(proxy, request\)\.await/,
  )
  assert.doesNotMatch(
    proxy,
    /#\[cfg\(feature = "mobile-store"\)\][\s\S]{0,80}(?:forward_local|route_to_remote|configure_local_runtime)/,
  )
  assert.match(startupPage, /data-phase="starting"/)
  assert.match(startupPage, /@keyframes runtime-progress/)
  assert.match(startupPage, /window\.setTimeout\(loadState, 300\)/)
  assert.match(shell, /mobile_retry_local_startup/)
  assert.match(shell, /on_web_content_process_terminate/)
  assert.match(shell, /STARTUP_PROBE_ATTEMPTS/)
  assert.match(shell, /_pisper_recovery/)
  assert.match(recovery, /visibilitychange/)
  assert.match(recovery, /mobile_resume_local_runtime/)
  assert.match(recovery, /_pisper_resume_probe/)
  assert.match(recovery, /window\.location\.replace/)
  assert.match(http, /await waitForMobileRuntimeReady\(\)/)
  assert.match(chat, /open: async[\s\S]*await waitForMobileRuntimeReady\(\)/)
  assert.match(startupPage, /mobile_retry_local_startup/)
  assert.match(permissions, /"mobile_retry_local_startup"/)
  assert.match(permissions, /"mobile_resume_local_runtime"/)
  assert.match(buildScript, /rerun-if-changed=permissions\/mobile\.toml/)
  assert.match(buildScript, /rerun-if-changed=capabilities\/mobile-bridge\.json/)
  assert.match(iosBuild, /tauri ios xcode-script/)
  assert.match(iosBuild, /rmSync\(generatedRustLibrary, \{ force: true \}\)/)
  assert.match(iosBuild, /'--features',[\s\S]*'mobile-embedded-only'/)
  assert.match(iosBuild, /mobile_resume_local_runtime/)
  assert.match(iosBuild, /assertResumeCommandAcl\(\)/)
  assert.doesNotMatch(startupPage, /mobile_enter_local/)
  assert.match(staticHandler, /const isAssetPath = requested === 'assets'/)
  assert.match(staticHandler, /静态资源不存在。/)
  assert.match(proxy, /MAX_FRONTEND_RESPONSE_BYTES/)
  assert.match(proxy, /response\.bytes\(\)/)
  assert.match(proxy, /timeout\(Duration::from_secs\(15\)\)/)
  assert.match(proxy, /读取 App 前端资源失败/)
  assert.match(shell, /mobile_recover_application/)
  assert.match(shell, /recovery_navigation_url/)
  assert.match(shell, /window\s*\.navigate\(recovery_navigation_url/)
  assert.match(shell, /moduleFailure/)
  assert.match(shell, /LONG_BACKGROUND_MS/)
  assert.match(permissions, /"mobile_recover_application"/)
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

test('Android 与 iOS 软键盘都使用可视视口保持会话输入框可见', async () => {
  const [app, navigation, focus, stabilizer, styles, html, setup, activity, iosPlugin] =
    await Promise.all([
      readFile('src/App.tsx', 'utf8'),
      readFile('src/components/layout/MobileNavigation.tsx', 'utf8'),
      readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
      readFile('src/components/layout/MobileViewportStabilizer.tsx', 'utf8'),
      readFile('src/index.css', 'utf8'),
      readFile('index.html', 'utf8'),
      readFile('scripts/setup-mobile-android.mjs', 'utf8'),
      readFile('src-tauri/mobile/android/MainActivity.kt', 'utf8'),
      readFile('src-tauri/mobile-device-plugin/ios/Sources/MobileDevicePlugin.swift', 'utf8'),
    ])

  assert.match(html, /interactive-widget=resizes-content/)
  assert.match(app, /MobileViewportStabilizer/)
  assert.match(stabilizer, /const viewport = window\.visualViewport/)
  assert.match(stabilizer, /viewport\?\.height \?\? window\.innerHeight/)
  assert.match(stabilizer, /viewport\?\.offsetTop \?\? 0/)
  assert.match(stabilizer, /let viewportBaseline = Math\.max\(window\.innerHeight/)
  assert.match(stabilizer, /shell\.style\.height = `\$\{height\}px`/)
  assert.match(
    stabilizer,
    /shell\.style\.transform = offsetTop \? `translate3d\(0, \$\{offsetTop\}px, 0\)` : ''/,
  )
  assert.match(stabilizer, /shell\.style\.height = ''/)
  assert.match(stabilizer, /viewportBaseline - height - offsetTop/)
  assert.match(
    stabilizer,
    /window\.addEventListener\('orientationchange', resetAfterOrientationChange\)/,
  )
  assert.match(stabilizer, /viewport\?\.addEventListener\('resize', schedule\)/)
  assert.match(stabilizer, /viewport\?\.addEventListener\('scroll', schedule\)/)
  assert.match(stabilizer, /shell\.dataset\.mobileKeyboard = 'open'/)
  assert.match(navigation, /\[\[data-mobile-keyboard='open'\]_&\]:pointer-events-none/)
  assert.match(navigation, /transition-\[max-height,opacity,border-color,padding-bottom\]/)
  assert.match(stabilizer, /document\.addEventListener\('touchstart', handleTouch, true\)/)
  assert.match(stabilizer, /document\.addEventListener\('pointerdown', handleTouch, true\)/)
  assert.match(stabilizer, /document\.addEventListener\('focusin', handleFocus, true\)/)
  assert.match(stabilizer, /document\.addEventListener\('focusout', handleBlur, true\)/)
  assert.match(stabilizer, /document\.addEventListener\('click', handleWelcomeClick, true\)/)
  assert.match(stabilizer, /markKeyboardTransition\('opening'\)/)
  assert.match(stabilizer, /markKeyboardTransition\('closing'\)/)
  assert.match(stabilizer, /closest\('\.agent-welcome button'\)/)
  assert.match(focus, /\[\[data-mobile-keyboard='open'\]_&\]:pointer-events-none/)
  assert.match(focus, /\[\[data-mobile-keyboard-transition='opening'\]_&\]:pointer-events-none/)
  assert.doesNotMatch(focus, /welcome-workspace[^"\n]*\[animation:/)
  assert.doesNotMatch(focus, /style=\{\{ animationDelay/)
  assert.doesNotMatch(
    styles,
    /data-mobile-composer|\.main-surface:has\(\.focus-composer textarea:focus\)/,
  )
  assert.doesNotMatch(styles, /display: none !important/)
  assert.match(stabilizer, /Android adjustResize/)
  assert.match(activity, /SOFT_INPUT_ADJUST_RESIZE/)
  assert.match(setup, /android:windowSoftInputMode="adjustResize"/)
  assert.match(iosPlugin, /keyboardDidShowNotification/)
  assert.match(iosPlugin, /keyboardDidChangeFrameNotification/)
  assert.match(iosPlugin, /keyboardDidHideNotification/)
  assert.doesNotMatch(iosPlugin, /keyboardWillShowNotification|keyboardWillHideNotification/)
  assert.match(iosPlugin, /document\.documentElement\.dataset\.mobileKeyboard/)
})

test('标准移动包只声明受控的联系人、相机、麦克风、照片、前台定位与局域网权限', async () => {
  const [
    setup,
    androidPlugin,
    androidManifest,
    androidActivity,
    voiceControl,
    voiceInput,
    mobileBridge,
    mobilePermissions,
    iosInfo,
  ] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('src-tauri/mobile-device-plugin/android/src/main/AndroidManifest.xml', 'utf8'),
    readFile('src-tauri/mobile/android/MainActivity.kt', 'utf8'),
    readFile('src/features/chat/VoiceInputControl.tsx', 'utf8'),
    readFile('src/features/chat/voice-input.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('src-tauri/Info.ios.plist', 'utf8'),
  ])
  for (const permission of [
    'android.permission.READ_CONTACTS',
    'android.permission.CAMERA',
    'android.permission.ACCESS_COARSE_LOCATION',
    'android.permission.ACCESS_FINE_LOCATION',
    'android.permission.READ_EXTERNAL_STORAGE',
    'android.permission.READ_MEDIA_IMAGES',
    'android.permission.READ_MEDIA_VISUAL_USER_SELECTED',
    'android.permission.VIBRATE',
    'android.permission.ACCESS_LOCAL_NETWORK',
    'android.permission.ACCESS_NETWORK_STATE',
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
  assert.match(iosInfo, /NSAppTransportSecurity[\s\S]*NSAllowsLocalNetworking[\s\S]*<true\/>/)
  assert.match(iosInfo, /NSCameraUsageDescription/)
  assert.match(iosInfo, /NSContactsUsageDescription/)
  assert.match(iosInfo, /NSLocationWhenInUseUsageDescription/)
  assert.match(iosInfo, /NSPhotoLibraryUsageDescription/)
  assert.match(iosInfo, /NSPhotoLibraryAddUsageDescription/)
  assert.doesNotMatch(iosInfo, /NSLocationAlways/)
  assert.match(setup, /android\.permission\.MODIFY_AUDIO_SETTINGS/)
  assert.match(androidManifest, /android\.permission\.RECORD_AUDIO/)
  assert.match(androidManifest, /android\.permission\.MODIFY_AUDIO_SETTINGS/)
  assert.match(androidActivity, /PermissionRequest\.RESOURCE_AUDIO_CAPTURE/)
  assert.match(androidActivity, /request\.grant\(audioResources\)/)
  assert.ok(
    voiceControl.indexOf('await requestMicrophonePermission()') <
      voiceControl.indexOf('await startMicrophoneCapture'),
  )
  assert.match(voiceInput, /'mobile_request_microphone_permission'/)
  assert.match(voiceInput, /'mobile_transcribe_pcm'/)
  assert.match(mobileBridge, /fn mobile_transcribe_pcm/)
  assert.match(mobileBridge, /\.transcribe_pcm\(pcm_base64\)/)
  assert.match(mobileBridge, /generate_handler!\[[\s\S]*mobile_transcribe_pcm/)
  assert.match(androidPlugin, /fun transcribePcm\(invoke: Invoke\)/)
  assert.match(mobilePermissions, /"mobile_request_microphone_permission"/)
  assert.match(mobilePermissions, /"mobile_transcribe_pcm"/)
  assert.match(iosInfo, /NSMicrophoneUsageDescription/)
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
    'apps.share_text',
  ]) {
    assert.match(`${tool}\n${client}\n${androidPlugin}\n${iosPlugin}`, new RegExp(operation))
  }
  assert.doesNotMatch(store, /DeviceCapabilities|device_capabilities/)
  assert.match(androidPlugin, /Intent\.ACTION_VIEW/)
  assert.match(androidPlugin, /Intent\.ACTION_DIAL/)
  assert.match(androidPlugin, /Intent\.ACTION_SENDTO/)
  assert.match(androidPlugin, /Intent\.ACTION_SEND/)
  assert.match(androidPlugin, /Intent\(Intent\.ACTION_MAIN\)[\s\S]*Intent\.CATEGORY_LAUNCHER/)
  assert.doesNotMatch(androidPlugin, /makeMainSelectorActivity/)
  assert.match(iosPlugin, /UIApplication\.shared\.open/)
  assert.match(tool, /FORBIDDEN_APP_SCHEMES/)
  assert.doesNotMatch(`${androidPlugin}\n${iosPlugin}`, /QUERY_ALL_PACKAGES|SEND_SMS|CALL_PHONE/)
})

test('受控移动设备协议在两端原生桥完整对齐', async () => {
  const [tool, client, rust, android, ios] = await Promise.all([
    readFile('runtime/tools/app/mobile-device.mjs', 'utf8'),
    readFile('src/features/chat/mobile-operations.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('src-tauri/mobile-device-plugin/ios/Sources/MobileDevicePlugin.swift', 'utf8'),
  ])
  for (const operation of [
    'device.info',
    'device.capabilities',
    'device.battery',
    'device.storage',
    'device.memory',
    'device.network',
    'device.display',
    'device.locale',
    'device.status',
    'device.clipboard.get',
    'device.clipboard.set',
    'device.vibrate',
    'device.flashlight',
    'device.notify',
    'photos.list',
    'photos.create_album',
    'photos.add_to_album',
    'photos.delete',
    'apps.share_text',
  ]) {
    assert.match(`${tool}\n${client}\n${rust}\n${android}\n${ios}`, new RegExp(operation))
  }
  assert.match(rust, /capability == "photos" && state == Some\("limited"\)/)
  assert.match(tool, /params\.action === 'delete_photos'[\s\S]*confirmed !== true/)
  assert.match(android, /MediaStore\.Images\.Media\.RELATIVE_PATH/)
  assert.match(ios, /PHPhotoLibrary\.shared\(\)\.performChanges/)
  assert.match(ios, /UIActivityViewController/)
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
  assert.match(native, /device\.capabilities/)
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
    storeWorkflow,
    setup,
    androidKeepRules,
  ] = await Promise.all([
    readFile('scripts/mobile-node-artifacts.json', 'utf8'),
    readFile('scripts/stage-mobile-node-android.mjs', 'utf8'),
    readFile('scripts/stage-mobile-node-ios.mjs', 'utf8'),
    readFile('scripts/build-mobile-node-ios.sh', 'utf8'),
    readFile('scripts/smoke-mobile-node-ios.sh', 'utf8'),
    readFile('scripts/mobile-node-ios-smoke-view-controller.m', 'utf8'),
    readFile('.github/workflows/release-app.yml', 'utf8'),
    readFile('.github/workflows/build-store-app.yml', 'utf8'),
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile('src-tauri/mobile/node-host/android/pisper-node-host.pro', 'utf8'),
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
  assert.equal(workflow.match(/--config src-tauri\/tauri\.mobile-ios\.conf\.json/g)?.length, 2)
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
  assert.match(setup, /pisper-node-host\.pro/)
  assert.match(androidKeepRules, /-keep class com\.lingkongran\.pisper\.EmbeddedNodeHost/)
  assert.match(androidKeepRules, /EmbeddedNodeHost\$Companion/)
  assert.match(setup, /libc\+\+_shared\.so/)
  assert.match(setup, /externalNativeBuild/)
  assert.match(
    workflow,
    /Prepare Android project with embedded Node[\s\S]*NDK_HOME:.*27\.3\.13750724/,
  )
  assert.match(workflow, /lib\/arm64-v8a\/libc\+\+_shared\.so/)
  assert.match(workflow, /android build --apk[\s\S]*--features mobile-embedded-only/)
  assert.match(
    workflow,
    /apkanalyzer dex packages --defined-only[\s\S]*EmbeddedNodeHost java\.lang\.String start/,
  )
  assert.doesNotMatch(
    workflow,
    /name: app-root-runtime|needs: \[[^\]]*root-runtime|cp [^\n]*pisper-root-runtime/,
  )
  assert.match(
    storeWorkflow,
    /stage-mobile-node-android\.mjs release\/mobile-node-android --require-sigstore/,
  )
  assert.match(
    storeWorkflow,
    /stage-mobile-node-ios\.mjs release\/mobile-node-ios --require-sigstore/,
  )
  assert.match(storeWorkflow, /--features mobile-store/)
  assert.doesNotMatch(
    storeWorkflow,
    /name: app-root-runtime|needs: \[[^\]]*root-runtime|cp [^\n]*pisper-root-runtime/,
  )
})

test('移动端明确区分远程档案与当前 Runtime 路由', async () => {
  await Promise.all([
    assert.rejects(readFile('public/connect.html'), /ENOENT/),
    assert.rejects(readFile('public/connect.js'), /ENOENT/),
  ])
  const [native, permissions, bridgeCapability, appPaths, settings, pairing] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('src-tauri/capabilities/mobile-bridge.json', 'utf8'),
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
  assert.match(pairing, /windowed: true/)
  assert.match(pairing, /invokeScanner<void>\('cancel'\)/)
  assert.match(pairing, /window\.addEventListener\('popstate', handleBack\)/)
  assert.match(pairing, /setTemporaryStyle\(root, 'visibility', 'hidden'\)/)
  assert.match(bridgeCapability, /barcode-scanner:allow-cancel/)
  assert.doesNotMatch(pairing, /mobile_pair_manual/)
  assert.match(pairing, /window\.location\.replace\(state\.proxyUrl\)/)
})

test('局域网发现需桌面审批且保留二维码备用路径', async () => {
  const [
    cargo,
    packageJson,
    native,
    pairing,
    remoteSettings,
    capability,
    mobilePermissions,
    iosInfo,
    mobileDeviceAndroid,
    dnsSdMobile,
    dnsSdAndroid,
    dnsSdAndroidManifest,
    dnsSdIos,
    dnsSdIosPermission,
    dnsSdIosPackage,
    dnsSdIosTests,
    ciWorkflow,
  ] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('package.json', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src/features/config/MobilePairingDialog.tsx', 'utf8'),
    readFile('src/features/config/RemoteAccessSettings.tsx', 'utf8'),
    readFile('src-tauri/capabilities/mobile-bridge.json', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('src-tauri/Info.ios.plist', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('crates/tauri-plugin-dns-sd/src/mobile.rs', 'utf8'),
    readFile(
      'crates/tauri-plugin-dns-sd/android/src/main/java/com/momics/dnssd/DnsSdPlugin.kt',
      'utf8',
    ),
    readFile('crates/tauri-plugin-dns-sd/android/src/main/AndroidManifest.xml', 'utf8'),
    readFile('crates/tauri-plugin-dns-sd/ios/Sources/DnsSdPlugin.swift', 'utf8'),
    readFile('crates/tauri-plugin-dns-sd/ios/Sources/LocalNetworkPermission.swift', 'utf8'),
    readFile('crates/tauri-plugin-dns-sd/ios/Package.swift', 'utf8'),
    readFile(
      'crates/tauri-plugin-dns-sd/ios/Tests/DnsSdPluginTests/DnsSdPluginTests.swift',
      'utf8',
    ),
    readFile('.github/workflows/ci.yml', 'utf8'),
  ])
  assert.match(cargo, /tauri-plugin-dns-sd = \{ path = "\.\.\/crates\/tauri-plugin-dns-sd" \}/)
  assert.match(dnsSdMobile, /#\[serde\(flatten\)\][\s\S]*options: BrowseOptions/)
  assert.match(dnsSdAndroid, /class BrowseStartArgs \{[\s\S]*var service: ServiceSpecData\?/)
  assert.match(dnsSdAndroid, /var timeoutMs: Long\?/)
  assert.doesNotMatch(dnsSdAndroid, /class BrowseStartArgs \{[\s\S]*?var options:/)
  assert.doesNotMatch(dnsSdAndroid, /serviceInfo\.serviceType != serviceType/)
  assert.doesNotMatch(dnsSdAndroid, /\.hostName/)
  assert.match(dnsSdAndroid, /removeSuffix\("\.local"\)/)
  assert.match(packageJson, /"@tauri-apps\/api": "\^2\.11\.1"/)
  assert.match(native, /plugin\(tauri_plugin_dns_sd::init\(\)\)/)
  assert.match(native, /mobile_ensure_local_network_permission/)
  assert.match(native, /mobile_pair_lan/)
  assert.match(native, /mobile_cancel_lan_pairing/)
  assert.match(mobilePermissions, /mobile_ensure_local_network_permission/)
  assert.match(mobilePermissions, /mobile_pair_lan/)
  assert.match(mobilePermissions, /mobile_cancel_lan_pairing/)
  assert.match(pairing, /mobile_ensure_local_network_permission/)
  assert.match(pairing, /plugin:dns-sd\|browse_start/)
  assert.match(pairing, /service: \{ type: 'pisper', protocol: 'tcp', domain: 'local' \}/)
  assert.match(pairing, /mobile_pair_lan/)
  assert.match(pairing, /mobile_cancel_lan_pairing/)
  assert.match(pairing, /plugin:barcode-scanner/)
  assert.doesNotMatch(pairing, /mobile_pair_manual/)
  assert.match(remoteSettings, /pendingApprovals/)
  assert.match(remoteSettings, /approval\.ip/)
  assert.match(remoteSettings, /decideApproval\(approval, true\)/)
  assert.match(remoteSettings, /decideApproval\(approval, false\)/)
  assert.doesNotMatch(remoteSettings, /status\.fingerprint/)
  assert.doesNotMatch(remoteSettings, /status\.endpoints/)
  assert.doesNotMatch(remoteSettings, /<RadioTower/)
  assert.match(capability, /dns-sd:default/)
  assert.match(mobileDeviceAndroid, /android\.permission\.ACCESS_LOCAL_NETWORK/)
  assert.match(mobileDeviceAndroid, /supportsLocalNetworkPermission/)
  assert.match(mobileDeviceAndroid, /getPermissionInfo\(/)
  assert.match(dnsSdAndroidManifest, /android\.permission\.CHANGE_WIFI_MULTICAST_STATE/)
  assert.match(iosInfo, /<key>NSLocalNetworkUsageDescription<\/key>/)
  assert.match(iosInfo, /discover and connect to a Pisper Desktop Runtime/)
  assert.match(iosInfo, /<key>NSBonjourServices<\/key>/)
  assert.match(iosInfo, /<string>_pisper\._tcp<\/string>/)
  assert.match(dnsSdIos, /bonjourWithTXTRecord/)
  assert.match(dnsSdIos, /NetService\(domain: domain, type: type, name: name\)/)
  assert.match(dnsSdIos, /NI_NUMERICHOST/)
  assert.match(dnsSdIos, /session\.resolvers\[key\] = resolver/)
  assert.match(dnsSdIos, /for \(_, resolver\) in session\.resolvers \{ resolver\.cancel\(\) \}/)
  assert.match(dnsSdIos, /case \.waiting\(let error\).*isLocalNetworkPermissionDenied/s)
  assert.match(dnsSdIos, /case \.cancelled:.*reason: "search-stopped"/s)
  assert.match(dnsSdIos, /reason: "permission-denied"/)
  assert.match(dnsSdIosPermission, /code == -65570/)
  assert.match(dnsSdIosPermission, /code == \.EACCES \|\| code == \.EPERM/)
  assert.match(dnsSdIosPackage, /\.testTarget\([\s\S]*name: "DnsSdPluginTests"/)
  assert.match(dnsSdIosTests, /isLocalNetworkPermissionDenied\(\.dns\(-65570\)\)/)
  assert.match(dnsSdIosTests, /XCTAssertFalse\(isLocalNetworkPermissionDenied/)
  assert.match(ciWorkflow, /bash scripts\/test-ios-dns-sd\.sh/)
  assert.match(pairing, /message\.reason === 'permission-denied'/)
  assert.match(pairing, /mobileServer\.localNetworkDenied/)
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

test('iOS 移动配置整体替换桌面资源且由两个发布通道共用', async () => {
  const [configText, releaseWorkflow, storeWorkflow, iosScript, iosBuildScript, packageText] =
    await Promise.all([
      readFile('src-tauri/tauri.mobile-ios.conf.json', 'utf8'),
      readFile('.github/workflows/release-app.yml', 'utf8'),
      readFile('.github/workflows/build-store-app.yml', 'utf8'),
      readFile('scripts/mobile-ios.mjs', 'utf8'),
      readFile('scripts/build-mobile-ios.mjs', 'utf8'),
      readFile('package.json', 'utf8'),
    ])
  const config = JSON.parse(configText)
  const packageJson = JSON.parse(packageText)

  assert.match(iosScript, /src-tauri['"], ['"]mobile-package\.json/)
  assert.match(iosScript, /JSON\.stringify\(\{ version: mobileVersion \}\)/)
  assert.equal(packageJson.scripts['init:android'], 'node scripts/setup-mobile-android.mjs')
  assert.equal(packageJson.scripts['build:android'], 'node scripts/build-mobile-android.mjs')
  assert.equal(packageJson.scripts['init:ios'], 'node scripts/mobile-ios.mjs init')
  assert.equal(packageJson.scripts['build:ios'], 'node scripts/build-mobile-ios.mjs')
  assert.equal(packageJson.scripts['android:init'], undefined)
  assert.equal(packageJson.scripts['android:apk'], undefined)
  assert.equal(packageJson.scripts['android:build'], undefined)
  assert.equal(packageJson.scripts['ios:init'], undefined)
  assert.equal(packageJson.scripts['ios:build'], undefined)
  assert.match(iosBuildScript, /build-frontend\.mjs/)
  assert.match(iosBuildScript, /build-mobile-runtime\.mjs/)
  assert.match(iosBuildScript, /PISPER_MOBILE_STORE: '0'/)
  assert.match(iosBuildScript, /sync-mobile-icons\.mjs/)
  assert.match(iosBuildScript, /pisper-embedded-runtime\.tar\.gz/)
  assert.deepEqual(config.bundle.externalBin, [])
  assert.deepEqual(config.bundle.resources, ['pisper-embedded-runtime.tar.gz'])
  assert.deepEqual(config.bundle.iOS.frameworks, [
    '../release/mobile-node-ios/NodeMobile.xcframework',
  ])
  for (const workflow of [releaseWorkflow, storeWorkflow]) {
    assert.match(workflow, /src-tauri\/pisper-embedded-runtime\.tar\.gz/)
    assert.match(workflow, /node scripts\/setup-mobile-ios\.mjs/)
    assert.match(workflow, /bash scripts\/test-ios-dns-sd\.sh/)
    assert.match(workflow, /App 根目录 PrivacyInfo|app bundle root/)
    assert.equal(workflow.match(/--config src-tauri\/tauri\.mobile-ios\.conf\.json/g)?.length, 2)
    assert.doesNotMatch(workflow, /release\/sea\/runtime/)
    assert.doesNotMatch(workflow, /resources":\{"\.\.\/release\/pisper-embedded-runtime/)
  }
})

test('iOS 插件兼容 Xcode 14 的 SwiftPM 与旧 SDK', async () => {
  const [packageManifest, plugin, setup] = await Promise.all([
    readFile('src-tauri/mobile-device-plugin/ios/Package.swift', 'utf8'),
    readFile('src-tauri/mobile-device-plugin/ios/Sources/MobileDevicePlugin.swift', 'utf8'),
    readFile('scripts/setup-mobile-ios.mjs', 'utf8'),
  ])

  assert.match(packageManifest, /^\/\/ swift-tools-version:5\.3/m)
  assert.match(plugin, /status\.rawValue ===? 4/)
  assert.doesNotMatch(plugin, /status == \.limited/)
  assert.match(setup, /Tests['"], ['"]TauriTests['"], ['"]TauriTests\.swift/)
})

test('iOS 隐私清单显式进入 App target 的资源构建阶段', () => {
  const project = `targets:\n  pisper-webview_iOS:\n    sources:\n      - path: Sources\n      - path: pisper-webview_iOS\n      - path: assets\n        buildPhase: resources\n`
  const updated = injectIosPrivacyManifest(project)

  assert.match(
    updated,
    /- path: pisper-webview_iOS\n      - path: \.\.\/\.\.\/PrivacyInfo\.xcprivacy\n        buildPhase: resources/,
  )
  assert.equal(injectIosPrivacyManifest(updated), updated)
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
  assert.match(rustHost, /installed_matches_packaged_runtime/)
  assert.match(rustHost, /archiveSha256/)
  assert.match(rustHost, /fn android_node_started\(\)/)
  assert.match(kotlin, /fun isStarted\(\): Boolean/)
  assert.match(kotlin, /finally \{[\s\S]*started\.set\(false\)/)
  assert.match(rustHost, /runtime_profile != runtime_profile\(\)/)
  assert.match(rustHost, /cfg!\(feature = "mobile-store"\)/)
  assert.match(rustHost, /"mobile-store"[\s\S]*"mobile-embedded"/)
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

test('商店构建在编译期排除 root Runtime 与外部安装更新', async () => {
  const [cargo, mobile, storeUpdate, workflow] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/src/mobile/store_update.rs', 'utf8'),
    readFile('.github/workflows/build-store-app.yml', 'utf8'),
  ])
  assert.match(cargo, /mobile-embedded-only = \[\]/)
  assert.match(cargo, /mobile-store = \["mobile-embedded-only"\]/)
  assert.match(mobile, /#\[cfg\(not\(feature = "mobile-embedded-only"\)\)\]\s*pub mod root_runtime/)
  assert.match(mobile, /#\[path = "store_update\.rs"\]/)
  assert.doesNotMatch(storeUpdate, /github\.com|latest-app\.json|open_url/)
  assert.match(workflow, /android build[\s\S]*--aab[\s\S]*--features mobile-store/)
  assert.match(workflow, /ios build[\s\S]*--features mobile-store/)
  assert.match(workflow, /--export-method app-store-connect/)
  assert.match(workflow, /tauri\.mobile-ios\.conf\.json/)
  assert.doesNotMatch(
    workflow,
    /name: app-root-runtime|needs: \[[^\]]*root-runtime|cp [^\n]*pisper-root-runtime/,
  )
})

test('公开移动文档与当前启动、配对、更新和隐私边界一致', async () => {
  const [readmeZh, readmeEn, mobileGuide, site, privacyMd, privacyHtml, support] =
    await Promise.all([
      readFile('README.md', 'utf8'),
      readFile('README.en.md', 'utf8'),
      readFile('docs/mobile.md', 'utf8'),
      readFile('docs/index.html', 'utf8'),
      readFile('docs/privacy.md', 'utf8'),
      readFile('docs/privacy.html', 'utf8'),
      readFile('docs/support.md', 'utf8'),
    ])
  const userDocs = `${readmeZh}\n${readmeEn}\n${mobileGuide}\n${site}`
  assert.doesNotMatch(userDocs, /首屏把|first screen presents|rooted Android|Linux chroot/)
  assert.match(readmeZh, /首次启动会直接进入内置的本机 Runtime/)
  assert.match(
    readmeEn,
    /On first launch, the Android \/ iOS app starts its bundled on-device Runtime/,
  )
  assert.match(mobileGuide, /桌面用户明确批准后，手机才领取一次性设备令牌/)
  assert.match(mobileGuide, /Google Play 与 App Store 构建只通过对应商店更新/)
  assert.match(site, /首次启动直接进入本机 Runtime/)
  for (const privacy of [privacyMd, privacyHtml]) {
    assert.match(privacy, /discover Desktop advertisements|discover and connect/)
    assert.match(privacy, /approve it before a device access token is issued/)
    assert.match(privacy, /GitHub build uses the same embedded Node Runtime and signed/)
  }
  assert.match(support, /select the discovered Desktop, and approve the request on Desktop/)
})

test('App 发布必须签名并校验共享 Runtime 与平台产物', async () => {
  const workflow = await readFile('.github/workflows/release-app.yml', 'utf8')
  for (const asset of [
    'pisper-embedded-runtime.tar.gz',
    'pisper-node-mobile-ios.tar.gz',
    'app-universal-release-signed.apk',
    'pisper-ios-unsigned.ipa',
  ]) {
    assert.match(workflow, new RegExp(`${asset.replaceAll('.', '\\.')}\\.minisig`))
  }
  assert.match(workflow, /pisper-embedded-runtime\.tgz/)
  assert.doesNotMatch(workflow, /pisper-root-runtime-android-arm64|app-root-runtime/)
  assert.match(workflow, /needs\['embedded-runtime'\]\.result == 'success'/)
  assert.doesNotMatch(workflow, /needs\['root-runtime'\]\.result/)
  assert.equal(workflow.match(/node scripts\/verify-tauri-signature\.mjs/g)?.length, 3)
  assert.doesNotMatch(workflow, /minisign -Vm/)
})
