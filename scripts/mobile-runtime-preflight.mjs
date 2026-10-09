import { existsSync } from 'node:fs'
import { join } from 'node:path'

// 在生成工程、下载原生依赖和覆盖旧归档前核验真正的移动 Runtime 入口。
export function assertMobileRuntimeSource(root) {
  if (existsSync(join(root, 'runtime/mobile-embedded.mjs'))) return
  if (existsSync(join(root, 'runtime-rs/Cargo.toml'))) {
    throw new Error(
      'MOBILE_RUNTIME_NOT_MIGRATED: 当前 Rust Runtime 尚未接入 Android/iOS 的进程内启动与关闭流程。' +
        '无法生成包含本机 Runtime 的移动包；请先完成移动端 Rust 集成。不会复用旧 Node 归档或生成空壳 APK/IPA。',
    )
  }
  throw new Error('MOBILE_RUNTIME_SOURCE_MISSING: 缺少 runtime/mobile-embedded.mjs。')
}
