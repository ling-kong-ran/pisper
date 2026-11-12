import { readdir, stat } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'

const tag = String(process.argv[2] || '').trim()
const artifactsDir = path.resolve(process.argv[3] || 'release/tauri-artifacts')
const match = tag.match(/^v(\d+\.\d+\.\d+)$/)
if (!match) {
  throw new Error(
    'Usage: node scripts/validate-tauri-release-assets.mjs v<version> [artifacts-dir]',
  )
}

const version = match[1]
// --platforms=：本机发布可以跳过某个平台（scripts/release-local.mjs），缺省时三个平台都要齐。
const platformsArg = process.argv.find((value) => value.startsWith('--platforms='))
const platforms = platformsArg
  ? platformsArg.slice('--platforms='.length).split(',').filter(Boolean)
  : ['darwin_aarch64', 'linux_x86_64', 'windows_x86_64']
const tauriAssets = [
  'latest.json',
  `Pisper_${version}_darwin_aarch64.app.tar.gz`,
  `Pisper_${version}_darwin_aarch64.app.tar.gz.sig`,
  `Pisper_${version}_darwin_aarch64.dmg`,
  // 壳后台下载 dmg 前要验这份签名（scripts/stage-tauri-artifacts.mjs 补签）。
  `Pisper_${version}_darwin_aarch64.dmg.sig`,
  `Pisper_${version}_linux_x86_64.AppImage`,
  `Pisper_${version}_linux_x86_64.AppImage.sig`,
  `Pisper_${version}_linux_x86_64.deb`,
  `Pisper_${version}_windows_x86_64-setup.exe`,
  `Pisper_${version}_windows_x86_64-setup.exe.sig`,
  `Pisper_${version}_windows_x86_64-offline-setup.exe`,
  `Pisper_${version}_windows_x86_64-offline-setup.exe.sig`,
].filter((name) => name === 'latest.json' || platforms.some((platform) => name.includes(platform)))
const componentAssets = platforms
  .map((platform) => `Pisper_Desktop_${version}_${platform}.tar.gz`)
  .flatMap((archive) => [archive, `${archive}.sig`])
const expected = new Set([...tauriAssets, ...componentAssets])

async function filesUnder(directory) {
  const result = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const fullPath = path.join(directory, entry.name)
    if (entry.isDirectory()) result.push(...(await filesUnder(fullPath)))
    else result.push(fullPath)
  }
  return result
}

const files = await filesUnder(artifactsDir)
const byName = new Map()
for (const file of files) {
  const name = path.basename(file)
  const existing = byName.get(name) || []
  existing.push(file)
  byName.set(name, existing)
}

const duplicates = [...byName].filter(([, entries]) => entries.length > 1).map(([name]) => name)
if (duplicates.length) {
  throw new Error(`Duplicate release asset names: ${duplicates.sort().join(', ')}.`)
}

const actual = new Set(byName.keys())
const missing = [...expected].filter((name) => !actual.has(name)).sort()
const unexpected = [...actual].filter((name) => !expected.has(name)).sort()
if (missing.length) throw new Error(`Missing release assets: ${missing.join(', ')}.`)
if (unexpected.length) throw new Error(`Unexpected release assets: ${unexpected.join(', ')}.`)

const empty = []
for (const [name, [file]] of byName) {
  if ((await stat(file)).size === 0) empty.push(name)
}
if (empty.length) throw new Error(`Empty release assets: ${empty.sort().join(', ')}.`)

console.log(`Validated ${actual.size} release assets for ${tag}.`)
