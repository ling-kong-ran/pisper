// Android 原生库打包方式：去掉调试符号并在 APK 内压缩存放。
// Tauri 模板把 keepDebugSymbols 写在 debug 块里，但 packaging 不属于构建类型，
// 实际改的是全局配置，release APK 因此带着全部符号表（libnode 一项就多 20 MB）。
// Android Studio 调试原生代码时读的是构建目录里未裁剪的副本，删掉它不影响调试。
// 库默认不压缩以便直接映射；APK 从 GitHub 下载，下载体积更要紧，所以改为压缩，
// 安装时由系统解压到 nativeLibraryDir，加载路径不变。
// sherpa AAR 附带的 C / C++ 接口库没有任何代码加载（Kotlin 只加载 JNI 库，JNI 库只依赖
// onnxruntime），一并排除。
const KEEP_DEBUG_SYMBOLS =
  /[ \t]*packaging \{[ \t]*(?:\r?\n[ \t]*)?(?:jniLibs\.keepDebugSymbols\.add\("[^"]+"\)\s*)+\}[ \t]*\r?\n/
const PACKAGING_MARKER = '// Pisper：原生库在 APK 内压缩存放'
const PACKAGING_BLOCK =
  /[ \t]*\/\/ Pisper：原生库在 APK 内压缩存放\r?\n[ \t]*packaging \{[\s\S]*?\r?\n {4}\}\r?\n/
const UNUSED_SHERPA_LIBRARIES = ['**/libsherpa-onnx-c-api.so', '**/libsherpa-onnx-cxx-api.so']
// 按需下载模式下语音原生库不进 APK（stage-android-speech-native.mjs）。
const DOWNLOADED_SPEECH_LIBRARIES = ['**/libonnxruntime.so', '**/libsherpa-onnx-jni.so']

export function configureAndroidNativePackaging(source, { downloadSpeechNative = false } = {}) {
  const eol = source.includes('\r\n') ? '\r\n' : '\n'
  // 每次重写整个块，切换语音原生库的打包方式时不会留下上一次的排除项。
  let next = source.replace(KEEP_DEBUG_SYMBOLS, '').replace(PACKAGING_BLOCK, '')
  if (next.includes('keepDebugSymbols')) {
    throw new Error('Android Gradle template keeps native debug symbols in an unknown form.')
  }
  if (/\buseLegacyPackaging\b/.test(next)) {
    throw new Error('Android Gradle template already sets useLegacyPackaging.')
  }
  if ([...next.matchAll(/^android \{/gm)].length !== 1) {
    throw new Error('Android Gradle template changed.')
  }
  const excludes = [
    ...UNUSED_SHERPA_LIBRARIES,
    ...(downloadSpeechNative ? DOWNLOADED_SPEECH_LIBRARIES : []),
  ]
  return next.replace(
    /^android \{/m,
    [
      'android {',
      `    ${PACKAGING_MARKER}`,
      '    packaging {',
      '        jniLibs {',
      '            useLegacyPackaging = true',
      `            excludes += setOf(${excludes.map((pattern) => JSON.stringify(pattern)).join(', ')})`,
      '        }',
      '    }',
    ].join(eol),
  )
}
