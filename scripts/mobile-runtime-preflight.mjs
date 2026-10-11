import { existsSync } from 'node:fs'
import { join } from 'node:path'

// 归档不能替代原生入口；在覆盖上次资源前检查库和宿主都在源树中。
export function assertMobileRuntimeSource(root) {
  for (const file of [
    'runtime-rs/Cargo.toml',
    'runtime-rs/src/lib.rs',
    'runtime-rs/src/runtime_host.rs',
    'src-tauri/src/mobile/embedded_runtime.rs',
  ]) {
    if (!existsSync(join(root, file))) throw new Error(`MOBILE_RUNTIME_SOURCE_MISSING: ${file}`)
  }
  return 'rust'
}
