import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import { join, dirname, delimiter } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { assertMobileRuntimeSource } from '../mobile-runtime-preflight.mjs'
import { createMobileRuntimeArchive } from '../mobile-runtime-archive.mjs'
import { x as extractTar } from 'tar'
import { verifyIosSpeechBundle } from '../verify-ios-speech-bundle.mjs'
import { resolveAndroidEnv } from '../android-env.mjs'
import { enableAndroidReleaseNativeStripping } from '../android-native-packaging.mjs'

for (const eol of ['\n', '\r\n']) {
  test(`Android Release strips native symbols without changing Debug settings (${JSON.stringify(eol)})`, () => {
    const source = [
      'android {',
      '    buildTypes {',
      '        getByName("debug") {',
      '            packaging { jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so") }',
      '        }',
      '    }',
      '}',
    ].join(eol)
    const patched = enableAndroidReleaseNativeStripping(source)
    assert.ok(patched.startsWith(source))
    assert.match(patched, /onVariants\(selector\(\)\.withBuildType\("release"\)\)/)
    assert.match(patched, /variant\.packaging\.jniLibs\.keepDebugSymbols\.set\(emptySet\(\)\)/)
    assert.equal(enableAndroidReleaseNativeStripping(patched), patched)
    assert.equal(patched.includes('\r\n'), eol === '\r\n')
  })
}

test('mobile speech resource aliases retain identical bytes on every checkout platform', async () => {
  for (const name of ['speech-model-catalog.json', 'speech-resource-notices.json']) {
    const base = new URL('../../shared/', import.meta.url)
    assert.deepEqual(
      await readFile(new URL(`speech/${name}`, base)),
      await readFile(new URL(name, base)),
    )
  }
})

for (const pathKey of ['PATH', 'Path']) {
  test(`Android toolchain preserves ${pathKey} and does not hide Cargo when adding Java`, () => {
    const source = {
      JAVA_HOME: '/fixture/jdk',
      ANDROID_HOME: '/fixture/sdk',
      NDK_HOME: '/fixture/ndk',
      [pathKey]: ['/fixture/cargo/bin', '/fixture/node/bin'].join(delimiter),
    }
    const first = resolveAndroidEnv(source)
    assert.deepEqual(
      Object.keys(first).filter((key) => key.toLowerCase() === 'path'),
      [pathKey],
    )
    assert.deepEqual(first[pathKey].split(delimiter), [
      join(source.JAVA_HOME, 'bin'),
      '/fixture/cargo/bin',
      '/fixture/node/bin',
    ])
    assert.equal(resolveAndroidEnv(first)[pathKey], first[pathKey])
    assert.equal(source[pathKey].includes('jdk'), false)
  })
}

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

test(
  'Android Linux bindgen preserves an explicitly configured host libclang',
  {
    skip: process.platform !== 'linux',
  },
  async () => {
    const { mobileRustEnv } = await import('../mobile-rust-env.mjs')
    const initial = {
      JAVA_HOME: '/fixture/jdk',
      ANDROID_HOME: '/fixture/sdk',
      NDK_HOME: '/fixture/ndk',
      PATH: '/fixture/bin',
      LIBCLANG_PATH: '/opt/host-llvm/lib',
    }
    const env = mobileRustEnv('android', initial)
    assert.equal(env.LIBCLANG_PATH, initial.LIBCLANG_PATH)
    assert.equal(mobileRustEnv('android', env).LIBCLANG_PATH, initial.LIBCLANG_PATH)
    assert.match(env.CC_aarch64_linux_android, /linux-x86_64\/bin\/aarch64-linux-android24-clang$/)
    assert.equal(
      env.BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android,
      '--sysroot="/fixture/ndk/toolchains/llvm/prebuilt/linux-x86_64/sysroot" --target=aarch64-linux-android24',
    )
  },
)

test(
  'Android Linux bindgen can discover system libclang when no override is configured',
  {
    skip: process.platform !== 'linux',
  },
  async () => {
    const { mobileRustEnv } = await import('../mobile-rust-env.mjs')
    const env = mobileRustEnv('android', {
      JAVA_HOME: '/fixture/jdk',
      ANDROID_HOME: '/fixture/sdk',
      NDK_HOME: '/fixture/ndk',
      PATH: '/fixture/bin',
    })
    assert.equal(Object.hasOwn(env, 'LIBCLANG_PATH'), false)
    assert.match(
      env.BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android,
      /--target=aarch64-linux-android24$/,
    )
  },
)

test('Android TLS staging refuses unpinned or corrupt certificate-verifier bytes', async () => {
  const { validateAndroidTls, ANDROID_TLS_RESOURCE } = await import('../stage-android-tls.mjs')
  assert.throws(
    () => validateAndroidTls(Buffer.alloc(ANDROID_TLS_RESOURCE.bytes)),
    /digest mismatch/,
  )
  assert.throws(() => validateAndroidTls(Buffer.from('untrusted')), /digest mismatch/)
})

test('Android bindgen preserves the host LLVM override instead of assuming NDK bundles libclang', async () => {
  const { mobileRustEnv } = await import('../mobile-rust-env.mjs')
  const source = {
    JAVA_HOME: '/fixture/jdk',
    ANDROID_HOME: '/fixture/sdk',
    NDK_HOME: '/fixture/nonexistent-ndk',
    PATH: '/fixture/bin',
  }
  assert.equal(Object.hasOwn(mobileRustEnv('android', source), 'LIBCLANG_PATH'), false)
  const override = { ...source, LIBCLANG_PATH: '/fixture/host-llvm' }
  assert.equal(mobileRustEnv('android', override).LIBCLANG_PATH, override.LIBCLANG_PATH)
})

test('Android bindgen preserves SDK paths containing spaces and Windows separators', async () => {
  const { mobileRustEnv } = await import('../mobile-rust-env.mjs')
  const env = mobileRustEnv('android', {
    JAVA_HOME: '/fixture/jdk',
    ANDROID_HOME: '/fixture/sdk',
    NDK_HOME: 'C:\\Android SDK\\ndk',
    PATH: '/fixture/bin',
  })
  assert.match(
    env.BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android,
    /^--sysroot="C:\/Android SDK\/ndk\/toolchains\/llvm\/prebuilt\/[^/]+\/sysroot" --target=aarch64-linux-android24$/,
  )
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

test('iOS packaged resources reject obsolete Node payloads while preserving native speech', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-ios-bundle-'))
  try {
    const appRoot = join(root, 'Pisper.app')
    const resources = join(appRoot, 'speech.bundle/SpeechResources')
    for (const name of [
      'speech-model-catalog.json',
      'speech-resource-notices.json',
      'speech-resources/xasr-bpe.vocab',
    ]) {
      for (const base of [join(root, 'shared'), resources]) {
        await mkdir(dirname(join(base, name)), { recursive: true })
        await writeFile(join(base, name), 'same shared fixture')
      }
    }
    assert.equal((await verifyIosSpeechBundle({ appRoot, root })).resourcesVerified, 3)
    for (const name of [
      'Frameworks/NodeMobile.framework/NodeMobile',
      'assets/NodeMobile.xcframework/Info.plist',
      'assets/node_modules/legacy/index.js',
      'assets/libnode.so',
    ]) {
      const stale = join(appRoot, name)
      await mkdir(dirname(stale), { recursive: true })
      await writeFile(stale, 'stale runtime')
      await assert.rejects(verifyIosSpeechBundle({ appRoot, root }), /legacy Node runtime/)
      await rm(stale)
    }
    assert.equal((await verifyIosSpeechBundle({ appRoot, root })).weightsBundled, false)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
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
