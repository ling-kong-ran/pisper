// Tauri 会把普通 bundle.resources 放入资源文件夹引用；App 级隐私清单必须单独进入
// iOS target 的 Resources build phase，才能稳定位于最终 .app 根目录。
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { load, dump } from 'js-yaml'
import { stageIosSpeechResources } from './stage-ios-speech-resources.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const defaultProjectSpec = join(root, 'src-tauri', 'gen', 'apple', 'project.yml')
const privacyManifest = join(root, 'src-tauri', 'PrivacyInfo.xcprivacy')
const privacyProjectPath = '../../PrivacyInfo.xcprivacy'
const generatedTauriApiRoots = [
  join(root, 'src-tauri', 'mobile-device-plugin', '.tauri', 'tauri-api'),
  join(root, 'crates', 'tauri-plugin-dns-sd', '.tauri', 'tauri-api'),
]

export function ensureGeneratedTauriApiTests() {
  for (const apiRoot of generatedTauriApiRoots) {
    const testSource = join(apiRoot, 'Tests', 'TauriTests', 'TauriTests.swift')
    if (existsSync(testSource)) continue
    mkdirSync(dirname(testSource), { recursive: true })
    writeFileSync(
      testSource,
      'import XCTest\n\nfinal class TauriTests: XCTestCase {\n  func testPackageLoads() {}\n}\n',
      'utf8',
    )
  }
}

export function injectIosPrivacyManifest(projectSpec) {
  if (projectSpec.includes(`- path: ${privacyProjectPath}`)) return projectSpec

  const appSourcePattern = /^(      - path: [^\r\n]+_iOS)\r?$/gm
  const appSources = [...projectSpec.matchAll(appSourcePattern)]
  if (appSources.length !== 1) {
    throw new Error(
      `无法在 iOS project.yml 中唯一定位 App target source，匹配数：${appSources.length}`,
    )
  }

  const eol = projectSpec.includes('\r\n') ? '\r\n' : '\n'
  const source = appSources[0][0]
  return projectSpec.replace(
    source,
    `${source}${eol}      - path: ${privacyProjectPath}${eol}        buildPhase: resources`,
  )
}

export function injectIosSystemConfigurationFramework(projectSpec) {
  if (projectSpec.includes('SystemConfiguration')) return projectSpec

  const eol = projectSpec.includes('\r\n') ? '\r\n' : '\n'
  const linkerSetting = `    OTHER_LDFLAGS: "$(inherited) -framework SystemConfiguration"`
  const baseSettings = new RegExp(`^settings${eol}  base${eol}`, 'm')
  if (baseSettings.test(projectSpec)) {
    return projectSpec.replace(baseSettings, `settings${eol}  base${eol}${linkerSetting}${eol}`)
  }

  const settings = new RegExp(`^settings${eol}`, 'm')
  if (settings.test(projectSpec)) {
    return projectSpec.replace(settings, `settings${eol}  base:${eol}${linkerSetting}${eol}`)
  }

  const targets = new RegExp(`^targets:${eol}`, 'm')
  if (!targets.test(projectSpec)) throw new Error('无法在 iOS project.yml 中定位 targets 配置')
  return projectSpec.replace(
    targets,
    `settings:${eol}  base:${eol}${linkerSetting}${eol}${eol}targets:${eol}`,
  )
}

export function injectIosNativeBuildSettings(projectSpec, minimumSystemVersion) {
  const project = load(projectSpec)
  const targets = Object.values(project.targets ?? {}).filter((target) => target.platform === 'iOS')
  if (targets.length === 0) throw new Error('无法在 iOS project.yml 中定位 iOS target')
  if (!/^\d+\.\d+(?:\.\d+)?$/.test(minimumSystemVersion)) {
    throw new Error('iOS minimumSystemVersion 无效')
  }
  project.options ??= {}
  project.options.deploymentTarget ??= {}
  project.options.deploymentTarget.iOS = minimumSystemVersion
  for (const target of targets) {
    for (const phase of target.preBuildScripts ?? []) {
      if (typeof phase.script === 'string' && phase.script.includes(' ios xcode-script ')) {
        // 直接以 node 调用 Tauri CLI 时，上游会生成错误的 `node tauri` 路径。
        const node = `'${process.execPath.replaceAll("'", "'\\''")}'`
        phase.script = phase.script.replace(
          /^.*?(?= ios xcode-script )/,
          `${node} "$PROJECT_DIR/../../../node_modules/@tauri-apps/cli/tauri.js"`,
        )
      }
    }
    // 重建旧生成工程时也移除 NodeMobile，避免把旧 framework 混进 Rust App。
    if (Array.isArray(target.dependencies)) {
      target.dependencies = target.dependencies.filter(
        (dependency) => !String(dependency.framework || '').includes('NodeMobile'),
      )
    }
    // 重生成工程时 Externals 已有多架构静态库，不能再被推断为同名 App 资源。
    for (const source of target.sources ?? []) {
      if (source.path === 'Externals') source.buildPhase = 'none'
    }
    target.settings ??= {}
    target.settings.base ??= {}
    const settings = target.settings.base
    const current = settings.OTHER_LDFLAGS ?? '$(inherited)'
    const flags = Array.isArray(current) ? [...current] : [String(current)]
    // Cargo staticlib 不传递系统动态库依赖，必须在最终 Xcode 链接阶段显式补齐。
    for (const library of ['c++', 'z', 'bz2', 'iconv', 'xml2']) {
      const flag = `-l${library}`
      if (!flags.some((value) => String(value).split(/\s+/).includes(flag))) flags.push(flag)
    }
    settings.OTHER_LDFLAGS = flags.join(' ')
    settings.IPHONEOS_DEPLOYMENT_TARGET = minimumSystemVersion
  }
  return dump(project, { lineWidth: -1, noRefs: true })
}

async function main() {
  const projectSpecPath = resolve(process.argv[2] || defaultProjectSpec)
  if (!existsSync(projectSpecPath)) {
    throw new Error(`iOS project.yml 不存在：${projectSpecPath}`)
  }
  if (!existsSync(privacyManifest)) {
    throw new Error(`iOS 隐私清单不存在：${privacyManifest}`)
  }

  await stageIosSpeechResources({ root })
  ensureGeneratedTauriApiTests()

  const current = readFileSync(projectSpecPath, 'utf8')
  const config = JSON.parse(
    readFileSync(join(root, 'src-tauri', 'tauri.mobile-ios.conf.json'), 'utf8'),
  )
  const updated = injectIosNativeBuildSettings(
    injectIosSystemConfigurationFramework(injectIosPrivacyManifest(current)),
    config.bundle.iOS.minimumSystemVersion,
  )
  writeFileSync(projectSpecPath, updated, 'utf8')

  const result = spawnSync('xcodegen', ['generate', '--spec', projectSpecPath], {
    cwd: dirname(projectSpecPath),
    stdio: 'inherit',
  })
  if (result.status !== 0) {
    throw new Error(`重新生成 iOS Xcode 工程失败，退出码：${result.status ?? 'unknown'}`)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await main()
}
