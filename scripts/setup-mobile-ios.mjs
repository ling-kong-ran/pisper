// Tauri 会把普通 bundle.resources 放入资源文件夹引用；App 级隐私清单必须单独进入
// iOS target 的 Resources build phase，才能稳定位于最终 .app 根目录。
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const defaultProjectSpec = join(root, 'src-tauri', 'gen', 'apple', 'project.yml')
const privacyManifest = join(root, 'src-tauri', 'PrivacyInfo.xcprivacy')
const privacyProjectPath = '../../PrivacyInfo.xcprivacy'

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

function main() {
  const projectSpecPath = resolve(process.argv[2] || defaultProjectSpec)
  if (!existsSync(projectSpecPath)) {
    throw new Error(`iOS project.yml 不存在：${projectSpecPath}`)
  }
  if (!existsSync(privacyManifest)) {
    throw new Error(`iOS 隐私清单不存在：${privacyManifest}`)
  }

  const current = readFileSync(projectSpecPath, 'utf8')
  const updated = injectIosPrivacyManifest(current)
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
  main()
}
