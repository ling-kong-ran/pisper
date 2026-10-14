// Android 工具链环境解析：优先使用进程已有的 JAVA_HOME/ANDROID_HOME/NDK_HOME；
// 缺失时回退到本机免安装工具链目录 ~/.pisper-android（绿色 JDK + cmdline-tools SDK）。
import { existsSync, readdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { delimiter, join } from 'node:path'

export function resolveAndroidEnv(env = process.env) {
  const result = { ...env }
  const base = join(homedir(), '.pisper-android')
  if (!result.JAVA_HOME && existsSync(base)) {
    const jdk = readdirSync(base).find((entry) => entry.startsWith('jdk-'))
    if (jdk) result.JAVA_HOME = join(base, jdk)
  }
  const sdk = join(base, 'sdk')
  if (!result.ANDROID_HOME && existsSync(sdk)) result.ANDROID_HOME = sdk
  if (!result.ANDROID_SDK_ROOT && result.ANDROID_HOME) {
    result.ANDROID_SDK_ROOT = result.ANDROID_HOME
  }
  if (!result.NDK_HOME && result.ANDROID_HOME) {
    const ndkDir = join(result.ANDROID_HOME, 'ndk')
    // 有多个 NDK 时取版本号最大的一个。
    const ndk = existsSync(ndkDir) && readdirSync(ndkDir).sort().at(-1)
    if (ndk) result.NDK_HOME = join(ndkDir, ndk)
  }
  if (result.JAVA_HOME) {
    // 展开 process.env 后失去 Windows 的大小写不敏感查询，不能用 PATH 覆盖已有 Path。
    const pathKey = Object.keys(result).find((key) => key.toLowerCase() === 'path') || 'PATH'
    const entries = (result[pathKey] || '').split(delimiter).filter(Boolean)
    const javaBin = join(result.JAVA_HOME, 'bin')
    result[pathKey] = [javaBin, ...entries.filter((entry) => entry !== javaBin)].join(delimiter)
  }
  return result
}

// 校验并在缺失时给出可操作的提示。
export function assertAndroidEnv(env) {
  const missing = ['JAVA_HOME', 'ANDROID_HOME', 'NDK_HOME'].filter((key) => !env[key])
  if (missing.length) {
    console.error(`缺少 Android 工具链环境变量：${missing.join(', ')}`)
    console.error('参考 docs/mobile.md §0 的构建链说明安装，或放置到 ~/.pisper-android。')
    process.exit(1)
  }
}
