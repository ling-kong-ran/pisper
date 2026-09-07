// 为 Android/iOS 嵌入式 Node 生成同一份可执行 Runtime 闭包；归档包含标准
// Runtime、生产 React 资源及生产依赖；仅携带共享语音 catalog/BPE，权重由用户主动下载。
import { cp, mkdir, readFile, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { stageRuntimeClosure } from './stage-runtime-closure.mjs'
import { createMobileRuntimeArchive } from './mobile-runtime-archive.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseDir = join(root, 'release')
const workDir = join(releaseDir, 'mobile-runtime')
const runtimeDir = join(workDir, 'runtime')
const manifestPath = join(workDir, 'runtime-size-manifest.json')
const output = join(releaseDir, 'pisper-embedded-runtime.tar.gz')
const mobilePackage = JSON.parse(
  await readFile(join(root, 'src-tauri', 'mobile-package.json'), 'utf8'),
)
const appVersion = process.env.PISPER_APP_VERSION || mobilePackage.version
const runtimeProfile = process.env.PISPER_MOBILE_STORE === '1' ? 'mobile-store' : 'mobile-embedded'

await rm(workDir, { recursive: true, force: true })
await mkdir(workDir, { recursive: true })
const manifest = await stageRuntimeClosure({
  root,
  runtimeDir,
  manifestPath,
  target: { platform: 'mobile', arch: 'arm64', libc: null },
  appVersion,
  includeSpeechModel: false,
})
await cp(join(root, 'dist'), join(runtimeDir, 'dist'), { recursive: true, force: true })
await createMobileRuntimeArchive({ runtimeDir, output, appVersion, runtimeProfile })
console.log(
  `Mobile Runtime staged: ${output} (${(manifest.runtime.afterPrune.bytes / 1024 / 1024).toFixed(1)} MiB before frontend)`,
)
