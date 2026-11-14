import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { brotliDecompressSync, constants as zlibConstants } from 'node:zlib'
import { unzipSync, zipSync } from 'fflate'
import { create as createTar } from 'tar'
import { configureAndroidNativePackaging } from '../../scripts/android-native-packaging.mjs'
import {
  verifyAndroidRuntimeAsset,
  writeAndroidRuntimeAsset,
} from '../../scripts/android-runtime-asset.mjs'
import { patchSherpaAar, stripLoadLibrary } from '../../scripts/sherpa-aar.mjs'
import {
  ANDROID_SPEECH_NATIVE_ARCHIVE,
  stageAndroidSpeechNative,
} from '../../scripts/stage-android-speech-native.mjs'
import { androidSpeechNativeDir } from '../../scripts/stage-android-speech-runtime.mjs'
import { traceRuntimeFiles } from '../../scripts/trace-runtime-files.mjs'

const TAURI_TEMPLATE = [
  'android {',
  '    compileSdk = 36',
  '    buildTypes {',
  '        getByName("debug") {',
  '            isDebuggable = true',
  '            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")',
  '                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")',
  '                jniLibs.keepDebugSymbols.add("*/x86/*.so")',
  '                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")',
  '            }',
  '        }',
  '        getByName("release") {',
  '            isMinifyEnabled = true',
  '        }',
  '    }',
  '}',
  '',
]

for (const eol of ['\n', '\r\n']) {
  test(`Android release APK strips and compresses native libraries (${JSON.stringify(eol)})`, () => {
    const source = TAURI_TEMPLATE.join(eol)
    const bundled = configureAndroidNativePackaging(source)
    // keepDebugSymbols 写在 debug 块里也作用于 release，必须整段去掉。
    assert.doesNotMatch(bundled, /keepDebugSymbols/)
    assert.match(bundled, /useLegacyPackaging = true/)
    assert.match(bundled, /libsherpa-onnx-c-api\.so/)
    assert.doesNotMatch(bundled, /libonnxruntime\.so/)
    assert.equal(configureAndroidNativePackaging(bundled), bundled)
    assert.equal(bundled.includes('\r\n'), eol === '\r\n')

    const download = configureAndroidNativePackaging(bundled, { downloadSpeechNative: true })
    assert.match(download, /"\*\*\/libonnxruntime\.so", "\*\*\/libsherpa-onnx-jni\.so"/)
    assert.equal((download.match(/useLegacyPackaging/g) || []).length, 1)
    // 切回随包安装时不能留下上一次的排除项。
    assert.equal(configureAndroidNativePackaging(download), bundled)
  })
}

test('Android native packaging rejects drifted Gradle templates', () => {
  assert.throws(() =>
    configureAndroidNativePackaging(
      'android {\n    packaging { jniLibs.keepDebugSymbols += "x" }\n}\n',
    ),
  )
  assert.throws(() =>
    configureAndroidNativePackaging(
      'android {\n    packaging { jniLibs { useLegacyPackaging = false } }\n}\n',
    ),
  )
  assert.throws(() => configureAndroidNativePackaging('androidOther {\n}\n'))
})

// 生成一个只有 <clinit> 的最小 class 文件：ldc "<library>"; invokestatic System.loadLibrary; return
function loadLibraryClass(library) {
  const pool = []
  const utf8 = (value) => {
    const bytes = Buffer.from(value)
    pool.push(Buffer.concat([Buffer.from([1, bytes.length >> 8, bytes.length & 0xff]), bytes]))
    return pool.length
  }
  const entry = (...bytes) => {
    pool.push(Buffer.from(bytes))
    return pool.length
  }
  const thisClass = entry(7, 0, utf8('com/k2fsa/sherpa/onnx/Sample'))
  const superClass = entry(7, 0, utf8('java/lang/Object'))
  const system = entry(7, 0, utf8('java/lang/System'))
  const name = utf8('loadLibrary')
  const descriptor = utf8('(Ljava/lang/String;)V')
  const nameAndType = entry(12, 0, name, 0, descriptor)
  const method = entry(10, 0, system, 0, nameAndType)
  const string = entry(8, 0, utf8(library))
  const clinit = utf8('<clinit>')
  const voidDescriptor = utf8('()V')
  const code = utf8('Code')
  const instructions = [0x12, string, 0xb8, method >> 8, method & 0xff, 0xb1]
  const body = Buffer.from([0, 1, 0, 0, 0, 0, 0, instructions.length, ...instructions, 0, 0, 0, 0])
  return Buffer.concat([
    Buffer.from([0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52, 0, pool.length + 1]),
    ...pool,
    Buffer.from([0, 0x21, 0, thisClass, 0, superClass, 0, 0, 0, 0, 0, 1]),
    Buffer.from([0, 0x08, 0, clinit, 0, voidDescriptor, 0, 1, 0, code]),
    Buffer.from([0, 0, 0, body.length]),
    body,
    Buffer.from([0, 0]),
  ])
}

test('sherpa classes no longer load their JNI library statically', () => {
  const original = loadLibraryClass('sherpa-onnx-jni')
  const { bytes, patched } = stripLoadLibrary(original)
  assert.equal(patched, 1)
  // 指令换成等长 nop，只剩 return；class 其余字节不变。
  const code = bytes.subarray(bytes.length - 2 - 4 - 6, bytes.length - 2 - 4)
  assert.deepEqual([...code], [0, 0, 0, 0, 0, 0xb1])
  assert.equal(bytes.length, original.length)
  assert.equal(stripLoadLibrary(loadLibraryClass('other-library')).patched, 0)
})

test('the staged sherpa AAR keeps only arm64 speech libraries and patched classes', () => {
  const library = (value) => Buffer.from(value)
  const aar = zipSync({
    'AndroidManifest.xml': library('<manifest/>'),
    'classes.jar': zipSync({
      'com/k2fsa/sherpa/onnx/Sample.class': loadLibraryClass('sherpa-onnx-jni'),
    }),
    'jni/arm64-v8a/libonnxruntime.so': library('onnxruntime'),
    'jni/arm64-v8a/libsherpa-onnx-jni.so': library('jni'),
    'jni/arm64-v8a/libsherpa-onnx-c-api.so': library('c-api'),
    'jni/x86_64/libonnxruntime.so': library('x86'),
  })
  const { aar: patched, libraries, patchedClasses } = patchSherpaAar(aar)
  assert.equal(patchedClasses, 1)
  const entries = unzipSync(patched)
  assert.deepEqual(
    Object.keys(entries)
      .filter((name) => name.startsWith('jni/'))
      .sort(),
    ['jni/arm64-v8a/libonnxruntime.so', 'jni/arm64-v8a/libsherpa-onnx-jni.so'],
  )
  assert.deepEqual(Object.keys(libraries).sort(), ['libonnxruntime.so', 'libsherpa-onnx-jni.so'])
  const classes = unzipSync(entries['classes.jar'])
  assert.equal(stripLoadLibrary(classes['com/k2fsa/sherpa/onnx/Sample.class']).patched, 0)

  // 上游换了加载方式（例如别的库名）时构建失败，而不是装进一个会自己 loadLibrary 的类。
  const drifted = zipSync({
    'classes.jar': zipSync({
      'com/k2fsa/sherpa/onnx/Sample.class': loadLibraryClass('sherpa-onnx-jni'),
      'com/k2fsa/sherpa/onnx/Other.class': loadLibraryClass('sherpa-onnx-other'),
    }),
    'jni/arm64-v8a/libonnxruntime.so': library('onnxruntime'),
    'jni/arm64-v8a/libsherpa-onnx-jni.so': library('jni'),
  })
  assert.throws(() => patchSherpaAar(drifted), /still loads native code/)
})

test('GitHub APK downloads speech native libraries from its own Release', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-speech-native-test-'))
  try {
    const source = androidSpeechNativeDir(root)
    await mkdir(source, { recursive: true })
    await writeFile(join(source, 'libonnxruntime.so'), 'onnxruntime')
    await writeFile(join(source, 'libsherpa-onnx-jni.so'), 'jni')
    const assetsDir = join(root, 'assets')
    const staged = await stageAndroidSpeechNative({
      root,
      assetsDir,
      mode: 'download',
      version: '1.2.3',
    })
    const manifest = JSON.parse(
      await readFile(join(assetsDir, 'speech-native-android.json'), 'utf8'),
    )
    const archive = await readFile(staged.archive)
    assert.equal(manifest.archive.sha256, createHash('sha256').update(archive).digest('hex'))
    assert.equal(manifest.archive.bytes, archive.length)
    const github = `https://github.com/ling-kong-ran/pisper/releases/download/app-v1.2.3/${ANDROID_SPEECH_NATIVE_ARCHIVE}`
    assert.deepEqual(manifest.archive.urls, [`https://ghfast.top/${github}`, github])
    assert.deepEqual(
      manifest.files.map(({ path, bytes }) => [path, bytes]),
      [
        ['libonnxruntime.so', 11],
        ['libsherpa-onnx-jni.so', 3],
      ],
    )
    const listing = execFileSync('tar', ['-tjf', staged.archive], { encoding: 'utf8' })
    assert.deepEqual(listing.trim().split('\n').sort(), [
      'speech-native/',
      'speech-native/libonnxruntime.so',
      'speech-native/libsherpa-onnx-jni.so',
    ])

    await assert.rejects(
      stageAndroidSpeechNative({ root, assetsDir, mode: 'download', version: 'dev' }),
      /needs the App version/,
    )
    assert.equal(await stageAndroidSpeechNative({ root, assetsDir, mode: 'bundled' }), null)
    assert.equal(existsSync(join(assetsDir, 'speech-native-android.json')), false)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

async function writeTree(root, files) {
  for (const [path, contents] of Object.entries(files)) {
    await mkdir(dirname(join(root, path)), { recursive: true })
    await writeFile(join(root, path), contents)
  }
}

test('mobile Runtime trimming keeps loaded files, licenses and Pi, and fails on a missed file', async () => {
  const runtimeDir = await mkdtemp(join(tmpdir(), 'pisper-trace-test-'))
  try {
    await writeTree(runtimeDir, {
      'runtime/entry.mjs': "export const load = () => import('used-package')\n",
      'node_modules/used-package/package.json':
        '{"name":"used-package","type":"module","main":"index.js"}',
      'node_modules/used-package/index.js': "export { value } from './value.js'\n",
      'node_modules/used-package/value.js': 'export const value = 1\n',
      'node_modules/used-package/unused.js': 'export const unused = 1\n',
      'node_modules/used-package/LICENSE': 'MIT',
      'node_modules/unused-package/package.json': '{"name":"unused-package","type":"module"}',
      'node_modules/unused-package/index.js': 'export default 1\n',
      'node_modules/@earendil-works/pi-sample/package.json':
        '{"name":"@earendil-works/pi-sample","type":"module"}',
      'node_modules/@earendil-works/pi-sample/dist/lazy.js': 'export const lazy = 1\n',
      'node_modules/@earendil-works/pi-sample/dist/theme.json': '{}',
    })
    const removed = await traceRuntimeFiles({ runtimeDir })
    assert.ok(removed.fileCount >= 2)
    const exists = (path) => existsSync(join(runtimeDir, path))
    assert.ok(exists('node_modules/used-package/value.js'))
    assert.ok(exists('node_modules/used-package/LICENSE'))
    assert.ok(exists('node_modules/unused-package/package.json'))
    assert.ok(!exists('node_modules/used-package/unused.js'))
    assert.ok(!exists('node_modules/unused-package/index.js'))
    // Pi 用拼接路径加载自己的模块，它的 dist 整个保留。
    assert.ok(exists('node_modules/@earendil-works/pi-sample/dist/lazy.js'))
    assert.ok(exists('node_modules/@earendil-works/pi-sample/dist/theme.json'))

    // 追踪看不见的拼接路径：裁剪前能加载、裁剪后不能，必须让构建失败。
    await writeTree(runtimeDir, {
      'node_modules/used-package/value.js': [
        "import { readFileSync } from 'node:fs'",
        "const { file } = JSON.parse(readFileSync(new URL('./config.json', import.meta.url), 'utf8'))",
        'export const value = (await import(new URL(file, import.meta.url).href)).hidden',
        '',
      ].join('\n'),
      'node_modules/used-package/config.json': '{"file":"./hidden.js"}',
      'node_modules/used-package/hidden.js': 'export const hidden = 1\n',
    })
    await assert.rejects(
      traceRuntimeFiles({ runtimeDir }),
      /trimming broke 1 module\(s\): used-package/,
    )
  } finally {
    await rm(runtimeDir, { recursive: true, force: true })
  }
})

test('Android Runtime asset is the signed tar in brotli with the manifest readable from its head', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-runtime-asset-test-'))
  try {
    const manifest = JSON.stringify({ schemaVersion: 1, appVersion: '1.2.3' })
    // 随机内容让首项之后的数据块足够大：不 flush 时开头 16 KB 解不出任何字节。
    const noise = randomBytes(512 * 1024).toString('base64')
    await writeTree(directory, {
      'runtime/embedded-runtime.json': manifest,
      'runtime/large.txt': noise,
    })
    const archive = join(directory, 'runtime.tar.gz')
    await createTar(
      { cwd: join(directory, 'runtime'), file: archive, gzip: true, portable: true },
      ['embedded-runtime.json', 'large.txt'],
    )
    const asset = join(directory, 'runtime.tar.br')
    await writeAndroidRuntimeAsset(archive, asset)
    await verifyAndroidRuntimeAsset(archive, asset)
    const head = (await readFile(asset)).subarray(0, 16 * 1024)
    const decoded = brotliDecompressSync(head, {
      finishFlush: zlibConstants.BROTLI_OPERATION_FLUSH,
    })
    assert.equal(decoded.subarray(0, 21).toString(), 'embedded-runtime.json')
    assert.ok(decoded.includes(Buffer.from(manifest)))

    await writeFile(asset, Buffer.from('not the same tar'))
    await assert.rejects(verifyAndroidRuntimeAsset(archive, asset))
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
