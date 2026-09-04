// 为 Android/iOS 嵌入式 Node 生成同一份可执行 Runtime 闭包；归档包含标准
// Runtime、生产 React 资源及生产依赖，不包含另一套移动业务实现。Android 模型
// 另行产出给 AssetManager，避免归档与 APK assets 各保存一份。
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { create as createTar } from 'tar'
import { stageRuntimeClosure } from './stage-runtime-closure.mjs'
import { stageSpeechModel } from './stage-speech-model.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseDir = join(root, 'release')
const workDir = join(releaseDir, 'mobile-runtime')
const runtimeDir = join(workDir, 'runtime')
const manifestPath = join(workDir, 'runtime-size-manifest.json')
const speechWorkDir = join(releaseDir, 'mobile-speech-model-stage')
const speechOutputDir = join(releaseDir, 'mobile-speech-model')
const output = join(releaseDir, 'pisper-embedded-runtime.tar.gz')
const mobilePackage = JSON.parse(
  await readFile(join(root, 'src-tauri', 'mobile-package.json'), 'utf8'),
)
const appVersion = process.env.PISPER_APP_VERSION || mobilePackage.version
const runtimeProfile = process.env.PISPER_MOBILE_STORE === '1' ? 'mobile-store' : 'mobile-embedded'

await Promise.all([
  rm(workDir, { recursive: true, force: true }),
  rm(speechWorkDir, { recursive: true, force: true }),
  rm(speechOutputDir, { recursive: true, force: true }),
])
await mkdir(workDir, { recursive: true })
const manifest = await stageRuntimeClosure({
  root,
  runtimeDir,
  manifestPath,
  target: { platform: 'mobile', arch: 'arm64', libc: null },
  appVersion,
  includeSpeechModel: false,
})
// Android 原生识别器通过 AssetManager 读取模型；单独产出可避免 Runtime 归档再携带一份。
await stageSpeechModel({ root, runtimeDir: speechWorkDir })
await cp(join(speechWorkDir, 'runtime', 'speech-model'), speechOutputDir, {
  recursive: true,
  force: true,
})
await rm(speechWorkDir, { recursive: true, force: true })
await cp(join(root, 'dist'), join(runtimeDir, 'dist'), { recursive: true, force: true })
await writeFile(
  join(runtimeDir, 'embedded-runtime.json'),
  `${JSON.stringify(
    {
      schemaVersion: 1,
      appVersion,
      runtimeProfile,
      entry: 'runtime/mobile-embedded.mjs',
    },
    null,
    2,
  )}\n`,
)
await rm(output, { force: true })
await createTar(
  {
    cwd: runtimeDir,
    file: output,
    gzip: true,
    portable: true,
    noMtime: true,
    prefix: '.',
  },
  ['.'],
)
console.log(
  `Mobile Runtime staged: ${output} (${(manifest.runtime.afterPrune.bytes / 1024 / 1024).toFixed(1)} MiB before frontend)`,
)
console.log(`Android speech model staged: ${speechOutputDir}`)
