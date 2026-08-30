// 按 GitHub iOS 构建顺序生成本地真机 IPA；本地签名由 Xcode 的开发团队配置负责。
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
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
const generatedRustLibrary = join(
  root,
  'src-tauri',
  'gen',
  'apple',
  'Externals',
  'arm64',
  'release',
  'libapp.a',
)
const rustBuildCommand =
  'npm run -- tauri ios xcode-script -v --platform ${PLATFORM_DISPLAY_NAME:?} --sdk-root ${SDKROOT:?} --framework-search-paths "${FRAMEWORK_SEARCH_PATHS:?}" --header-search-paths "${HEADER_SEARCH_PATHS:?}" --gcc-preprocessor-definitions "${GCC_PREPROCESSOR_DEFINITIONS:-}" --configuration ${CONFIGURATION:?} ${FORCE_COLOR} ${ARCHS:?}'
const resumeAclSequence = Buffer.from(
  'allow-pisper-mobile-bridgemobile_statemobile_retry_local_startupmobile_resume_local_runtime',
)

function run(args, extraEnv = {}) {
  const result = spawnSync(node, args, {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, ...extraEnv },
  })
  if (result.status !== 0) process.exit(result.status ?? 1)
}

function restoreRustBuildPhase() {
  const project = readFileSync(generatedProject, 'utf8')
  if (project.includes('tauri ios xcode-script')) return

  const disabled = 'shellScript = "/usr/bin/true";'
  if (!project.includes(disabled)) {
    throw new Error('iOS Xcode 工程缺少可恢复的 Rust build phase。')
  }
  writeFileSync(
    generatedProject,
    project.replace(disabled, `shellScript = ${JSON.stringify(rustBuildCommand)};`),
    'utf8',
  )
}

function assertResumeCommandAcl() {
  if (!existsSync(generatedRustLibrary)) {
    throw new Error(`iOS Rust 静态库不存在：${generatedRustLibrary}`)
  }
  if (!readFileSync(generatedRustLibrary).includes(resumeAclSequence)) {
    throw new Error('iOS Rust 静态库缺少 mobile_resume_local_runtime ACL。')
  }
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
restoreRustBuildPhase()
run(['scripts/sync-mobile-icons.mjs'])
mkdirSync(dirname(generatedRuntimeArchive), { recursive: true })
copyFileSync(runtimeArchive, generatedRuntimeArchive)

// 缺失库会迫使 Xcode 运行 Rust 阶段，避免把旧 ACL 静态库链接进新 IPA。
rmSync(generatedRustLibrary, { force: true })
run([
  'scripts/mobile-ios.mjs',
  'build',
  '--target',
  'aarch64',
  '--features',
  'mobile-embedded-only',
  '--export-method',
  'debugging',
])
assertResumeCommandAcl()
if (!existsSync(generatedIpa)) throw new Error(`未找到构建出的 IPA：${generatedIpa}`)
mkdirSync(dirname(localIpa), { recursive: true })
copyFileSync(generatedIpa, localIpa)
console.log(`本地签名 IPA：${localIpa}`)
