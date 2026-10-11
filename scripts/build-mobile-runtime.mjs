// 移动端将 Rust Runtime 静态链接进 App；归档仅携带同源 UI 与共享资源。
import { assertMobileRuntimeSource } from './mobile-runtime-preflight.mjs'
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createMobileRuntimeArchive } from './mobile-runtime-archive.mjs'
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
assertMobileRuntimeSource(root)
const runtimeDir = join(root, 'release/mobile-runtime/runtime')
const output = join(root, 'release/pisper-embedded-runtime.tar.gz')
const appVersion =
  process.env.PISPER_APP_VERSION ||
  JSON.parse(await readFile(join(root, 'src-tauri/mobile-package.json'), 'utf8')).version
const runtimeProfile = process.env.PISPER_MOBILE_STORE === '1' ? 'mobile-store' : 'mobile-embedded'
await rm(runtimeDir, { recursive: true, force: true })
await mkdir(join(runtimeDir, 'shared'), { recursive: true })
await cp(join(root, 'dist'), join(runtimeDir, 'dist'), { recursive: true })
for (const entry of ['speech', 'speech-resources']) {
  await cp(join(root, 'shared', entry), join(runtimeDir, 'shared', entry), { recursive: true })
}
// 原生插件仍消费平铺路径；两种路径必须来自同一目录内容。
for (const name of ['speech-model-catalog.json', 'speech-resource-notices.json']) {
  const canonical = await readFile(join(root, 'shared/speech', name))
  if (!canonical.equals(await readFile(join(root, 'shared', name))))
    throw new Error(`Speech resource aliases diverged: ${name}`)
  await writeFile(join(runtimeDir, 'shared', name), canonical)
}
await writeFile(
  join(runtimeDir, 'runtime.json'),
  JSON.stringify({ backend: 'rust', apiVersion: 1, appVersion, runtimeProfile }) + '\n',
)
await createMobileRuntimeArchive({ runtimeDir, output, appVersion, runtimeProfile })
console.log(`Staged mobile Rust resources: ${output}`)
