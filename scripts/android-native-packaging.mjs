const releaseStripping = `// Pisper：Tauri 模板的 Debug packaging 会影响全局，按 Release 变体恢复符号剥离。
androidComponents {
    onVariants(selector().withBuildType("release")) { variant ->
        variant.packaging.jniLibs.keepDebugSymbols.set(emptySet())
    }
}`

export function enableAndroidReleaseNativeStripping(gradle) {
  const eol = gradle.includes('\r\n') ? '\r\n' : '\n'
  const block = releaseStripping.replaceAll('\n', eol)
  if (gradle.includes(block)) return gradle
  return `${gradle.trimEnd()}${eol}${eol}${block}${eol}`
}
