import { execFileSync } from 'node:child_process'
import { join } from 'node:path'
import { resolveAndroidEnv } from './android-env.mjs'

export const ANDROID_WEBVIEW_RECOVERY = `override fun onRenderProcessGone(
    view: WebView,
    detail: RenderProcessGoneDetail
): Boolean {
    return MainActivity.recoverFromRendererCrash(view, currentUrl, detail.didCrash())
}`

// QuickJS bindgen 必须使用目标 SDK，不能继承另一平台遗留的 libclang/头文件。
export function mobileRustEnv(platform, source = process.env) {
  const env = { ...source }
  if (platform === 'ios') {
    const xcrun = (args) => execFileSync('xcrun', args, { encoding: 'utf8' }).trim()
    env.LIBCLANG_PATH = join(
      execFileSync('xcode-select', ['-p'], { encoding: 'utf8' }).trim(),
      'Toolchains/XcodeDefault.xctoolchain/usr/lib',
    )
    for (const [target, sdk] of [
      ['aarch64_apple_ios', 'iphoneos'],
      ['aarch64_apple_ios_sim', 'iphonesimulator'],
      ['x86_64_apple_ios', 'iphonesimulator'],
    ]) {
      env[`BINDGEN_EXTRA_CLANG_ARGS_${target}`] =
        `-isysroot ${xcrun(['--sdk', sdk, '--show-sdk-path'])}`
    }
  } else if (platform === 'android') {
    Object.assign(env, resolveAndroidEnv(source))
    if (!env.NDK_HOME) throw new Error('Android NDK_HOME is required')
    const host =
      process.platform === 'darwin'
        ? 'darwin-x86_64'
        : process.platform === 'win32'
          ? 'windows-x86_64'
          : 'linux-x86_64'
    const toolchain = join(env.NDK_HOME, 'toolchains/llvm/prebuilt', host)
    const suffix = process.platform === 'win32' ? '.cmd' : ''
    // 与 Tauri Android 的 minSdk=24 一致，不能让 C 依赖隐式提高最低系统版本。
    const cc = join(toolchain, 'bin', `aarch64-linux-android24-clang${suffix}`)
    env.CC_aarch64_linux_android = cc
    env.AR_aarch64_linux_android = join(
      toolchain,
      'bin',
      `llvm-ar${process.platform === 'win32' ? '.exe' : ''}`,
    )
    env.CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = cc
    const flags = env.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS || ''
    env.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = flags.includes('-Wl,-z,max-page-size=16384')
      ? flags
      : `-C link-arg=-Wl,-z,max-page-size=16384 ${flags}`.trim()
    // Tauri 外层会追加系统库，内层从 IPC 恢复旧 target flags；用相同的编码参数消除两次构建的差异。
    // 保留用户显式的全局或带引号参数，不自行解释复杂 shell 转义。
    if (
      !env.CARGO_ENCODED_RUSTFLAGS &&
      !env.RUSTFLAGS &&
      !/["'\\]/.test(env.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS)
    ) {
      const args = env.CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS.split(/\s+/)
      for (const arg of ['-Clink-arg=-landroid', '-Clink-arg=-llog', '-Clink-arg=-lOpenSLES'])
        if (!args.includes(arg)) args.push(arg)
      env.CARGO_ENCODED_RUSTFLAGS = args.join('\x1f')
    }
    env.WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION = ANDROID_WEBVIEW_RECOVERY
    env.LIBCLANG_PATH = join(toolchain, process.platform === 'win32' ? 'bin' : 'lib')
    env.BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android = `--sysroot=${join(toolchain, 'sysroot')} --target=aarch64-linux-android24`
  } else throw new Error(`Unsupported mobile platform: ${platform}`)
  return env
}
