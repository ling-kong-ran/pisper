const DEPENDENCY = 'coreLibraryDesugaring("com.android.tools:desugar_jdk_libs_nio:2.1.5")'

export function enableAndroidSpeechDesugaring(source) {
  const eol = source.includes('\r\n') ? '\r\n' : '\n'
  let next = source
  if (/isCoreLibraryDesugaringEnabled\s*=\s*false/.test(next)) {
    throw new Error('Android speech requires core library desugaring.')
  }
  if (!/isCoreLibraryDesugaringEnabled\s*=\s*true/.test(next)) {
    const blocks = [...next.matchAll(/\bcompileOptions\s*\{/g)]
    if (blocks.length > 1) throw new Error('Ambiguous Android compileOptions blocks.')
    if (blocks.length === 1) {
      next = next.replace(
        /\bcompileOptions\s*\{/,
        `compileOptions {${eol}        isCoreLibraryDesugaringEnabled = true`,
      )
    } else {
      if ([...next.matchAll(/^android \{/gm)].length !== 1)
        throw new Error('Android Gradle template changed.')
      // Commons Compress 的 FileTime 路径需要 NIO 回补，不能只在新 Android 设备上验证。
      next = next.replace(
        /^android \{/m,
        [
          'android {',
          '    compileOptions {',
          '        sourceCompatibility = JavaVersion.VERSION_1_8',
          '        targetCompatibility = JavaVersion.VERSION_1_8',
          '        isCoreLibraryDesugaringEnabled = true',
          '    }',
        ].join(eol),
      )
    }
  }
  if (!next.includes(DEPENDENCY)) {
    if (/coreLibraryDesugaring\s*\(/.test(next))
      throw new Error('Android speech requires the pinned NIO desugaring library.')
    if ([...next.matchAll(/^dependencies \{/gm)].length !== 1)
      throw new Error('Android dependency template changed.')
    next = next.replace(/^dependencies \{/m, `dependencies {${eol}    ${DEPENDENCY}`)
  }
  return next
}
