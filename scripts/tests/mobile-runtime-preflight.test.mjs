import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import { join, dirname } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { assertMobileRuntimeSource } from '../mobile-runtime-preflight.mjs'
import { createMobileRuntimeArchive } from '../mobile-runtime-archive.mjs'
import { x as extractTar } from 'tar'

// 保留“缺少宿主时不能覆盖旧资源”的保护，成功合同改为 Rust 库与 v2 静态资源。
test('incomplete Rust mobile sources fail without modifying the previous archive', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-preflight-'))
  try {
    await mkdir(join(root, 'release'))
    const archive = join(root, 'release/pisper-embedded-runtime.tar.gz')
    await writeFile(archive, 'previous-archive')
    const sources = [
      'runtime-rs/Cargo.toml',
      'runtime-rs/src/lib.rs',
      'runtime-rs/src/runtime_host.rs',
      'src-tauri/src/mobile/embedded_runtime.rs',
    ]
    for (const file of sources) {
      assert.throws(() => assertMobileRuntimeSource(root), /MOBILE_RUNTIME_SOURCE_MISSING/)
      assert.equal(await readFile(archive, 'utf8'), 'previous-archive')
      await mkdir(dirname(join(root, file)), { recursive: true })
      await writeFile(join(root, file), 'fixture')
    }
    assert.equal(assertMobileRuntimeSource(root), 'rust')
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test('mobile archive binds Rust resources and UI to a deterministic v2 manifest', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-resources-'))
  try {
    const runtimeDir = join(root, 'runtime')
    await mkdir(join(runtimeDir, 'dist'), { recursive: true })
    await writeFile(join(runtimeDir, 'runtime.json'), JSON.stringify({ backend: 'rust' }))
    await writeFile(join(runtimeDir, 'dist/index.html'), '<title>fixture</title>')
    const output = join(root, 'runtime.tar.gz')
    const options = { runtimeDir, output, appVersion: '1.2.3', runtimeProfile: 'mobile-embedded' }
    const first = await createMobileRuntimeArchive(options)
    const second = await createMobileRuntimeArchive(options)
    assert.deepEqual(second, first)
    assert.equal(first.schemaVersion, 2)
    assert.equal(first.entry, 'runtime.json')
    const unpacked = join(root, 'unpacked')
    await mkdir(unpacked)
    await extractTar({ file: output, cwd: unpacked })
    assert.equal(JSON.parse(await readFile(join(unpacked, 'runtime.json'), 'utf8')).backend, 'rust')
    await writeFile(join(runtimeDir, 'dist/index.html'), '<title>changed</title>')
    const changed = await createMobileRuntimeArchive(options)
    assert.notEqual(changed.buildSha256, first.buildSha256)
    assert.notEqual(changed.frontendSha256, first.frontendSha256)
    await assert.rejects(
      createMobileRuntimeArchive({ ...options, runtimeProfile: 'desktop' }),
      /Invalid/,
    )
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test('Android build environment keeps the 16 KB linker flag idempotent across Tauri and Gradle', async () => {
  const { mobileRustEnv, ANDROID_WEBVIEW_RECOVERY } = await import('../mobile-rust-env.mjs')
  const initial = {
    JAVA_HOME: '/fixture/jdk',
    ANDROID_HOME: '/fixture/sdk',
    NDK_HOME: '/fixture/ndk',
    PATH: '/fixture/bin',
  }
  const first = mobileRustEnv('android', initial)
  const second = mobileRustEnv('android', first)
  assert.equal(
    first.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS,
    second.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS,
  )
  assert.equal(
    (second.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS.match(/max-page-size=16384/g) || [])
      .length,
    1,
  )
  assert.equal(first.WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION, ANDROID_WEBVIEW_RECOVERY)
})

test('Android TLS staging refuses unpinned or corrupt certificate-verifier bytes', async () => {
  const { validateAndroidTls, ANDROID_TLS_RESOURCE } = await import('../stage-android-tls.mjs')
  assert.throws(
    () => validateAndroidTls(Buffer.alloc(ANDROID_TLS_RESOURCE.bytes)),
    /digest mismatch/,
  )
  assert.throws(() => validateAndroidTls(Buffer.from('untrusted')), /digest mismatch/)
})

test('iOS generator repairs direct-node build commands and removes obsolete Node frameworks', async () => {
  const { injectIosNativeBuildSettings } = await import('../setup-mobile-ios.mjs')
  const { load, dump } = await import('js-yaml')
  const source = {
    targets: {
      App: {
        platform: 'iOS',
        dependencies: [{ framework: 'NodeMobile.xcframework' }, { sdk: 'UIKit.framework' }],
        preBuildScripts: [
          {
            script: 'node tauri ios xcode-script --configuration ${CONFIGURATION:?}',
            name: 'Build Rust Code',
          },
        ],
      },
    },
  }
  const fixed = load(injectIosNativeBuildSettings(dump(source), '15.1')).targets.App
  assert.match(fixed.preBuildScripts[0].script, /node_modules\/@tauri-apps\/cli\/tauri\.js/)
  assert.doesNotMatch(fixed.preBuildScripts[0].script, /node tauri ios/)
  assert.deepEqual(fixed.dependencies, [{ sdk: 'UIKit.framework' }])
})

test('Wry fallback injects only when document-start injection is unavailable and remains idempotent', async () => {
  const { transformWryAndroidWebViewClient } = await import('../patch-wry-android.mjs')
  const source = [
    '    override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {',
    '        if (interceptedState[url] == false) {',
    '            val webView = view as RustWebView',
    '            for (script in webView.initScripts) { view.evaluateJavascript(script, null) }',
    '        }',
    '    }',
    '    override fun onPageFinished(view: WebView, url: String) {',
    '        Rust.onPageLoaded((view as RustWebView).id, url)',
    '    }',
  ].join('\n')
  const result = transformWryAndroidWebViewClient(source)
  assert.match(
    result,
    /!\(view as RustWebView\)\.isDocumentStartScriptEnabled && interceptedState\[url\] == false/,
  )
  assert.equal(transformWryAndroidWebViewClient(result), result)
  assert.throws(
    () =>
      transformWryAndroidWebViewClient(
        source.replace('interceptedState[url] == false', 'changedUpstreamCondition'),
      ),
    /初始化脚本/,
  )
})
