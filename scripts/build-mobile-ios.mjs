// 按 GitHub iOS 构建顺序生成本地真机 IPA；本地签名由 Xcode 的开发团队配置负责。
import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const node = process.execPath
const mobileVersion = JSON.parse(
  readFileSync(join(root, 'src-tauri', 'mobile-package.json'), 'utf8'),
).version
const runtimeArchive = join(root, 'release', 'pisper-embedded-runtime.tar.gz')
const sourceRuntimeArchive = join(root, 'src-tauri', 'pisper-embedded-runtime.tar.gz')
const generatedRuntimeArchive = join(
  root,
  'src-tauri',
  'gen',
  'apple',
  'assets',
  'pisper-embedded-runtime.tar.gz',
)
const generatedIpa = join(root, 'src-tauri', 'gen', 'apple', 'build', 'arm64', 'Pisper.ipa')
const localIpa = join(root, 'release', `pisper-ios-${mobileVersion}-signed.ipa`)
const generatedProject = join(
  root,
  'src-tauri',
  'gen',
  'apple',
  'pisper-webview.xcodeproj',
  'project.pbxproj',
)

function run(args, extraEnv = {}) {
  const result = spawnSync(node, args, {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, ...extraEnv },
  })
  if (result.status !== 0) process.exit(result.status ?? 1)
}

run(['scripts/build-frontend.mjs'])
run(['scripts/check-bundle-budget.mjs'])
run(['scripts/build-mobile-runtime.mjs'], { PISPER_MOBILE_STORE: '0' })

if (!existsSync(runtimeArchive)) {
  throw new Error(`未找到嵌入式 Runtime 归档：${runtimeArchive}`)
}
copyFileSync(runtimeArchive, sourceRuntimeArchive)

// GitHub 流程先生成图标再初始化工程；已有工程也要直接覆盖资源，避免沿用 Tauri 默认图标。
run(['scripts/sync-mobile-icons.mjs'])
if (!existsSync(generatedProject)) run(['scripts/mobile-ios.mjs', 'init'])
run(['scripts/sync-mobile-icons.mjs'])
mkdirSync(dirname(generatedRuntimeArchive), { recursive: true })
copyFileSync(runtimeArchive, generatedRuntimeArchive)

run(['scripts/mobile-ios.mjs', 'build', '--target', 'aarch64', '--export-method', 'debugging'])
if (!existsSync(generatedIpa)) throw new Error(`未找到构建出的 IPA：${generatedIpa}`)
mkdirSync(dirname(localIpa), { recursive: true })
copyFileSync(generatedIpa, localIpa)
console.log(`本地签名 IPA：${localIpa}`)
