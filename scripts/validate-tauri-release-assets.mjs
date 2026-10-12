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
// 安装包和 updater 缺少任一已支持平台，都不能发布不完整的 latest.json。
const tauriAssets = [
  'latest.json',
  `Pisper_${version}_windows_x86_64-setup.exe`,
  `Pisper_${version}_windows_x86_64-setup.exe.sig`,
  `Pisper_${version}_windows_x86_64-offline-setup.exe`,
  `Pisper_${version}_windows_x86_64-offline-setup.exe.sig`,
  ...['aarch64', 'x86_64'].flatMap((arch) => [
    `Pisper_${version}_darwin_${arch}.dmg`,
    `Pisper_${version}_darwin_${arch}.app.tar.gz`,
    `Pisper_${version}_darwin_${arch}.app.tar.gz.sig`,
  ]),
  `Pisper_${version}_linux_x86_64.AppImage`,
  `Pisper_${version}_linux_x86_64.AppImage.sig`,
  `Pisper_${version}_linux_x86_64.deb`,
  ...['windows_x86_64', 'darwin_aarch64', 'darwin_x86_64', 'linux_x86_64'].flatMap((target) => [
    `Pisper_Desktop_Rust_${version}_${target}.tar.gz`,
    `Pisper_Desktop_Rust_${version}_${target}.tar.gz.sig`,
    `Pisper_Desktop_${version}_${target}.tar.gz`,
    `Pisper_Desktop_${version}_${target}.tar.gz.sig`,
  ]),
]
const expected = new Set(tauriAssets)

async function filesUnder(directory) {
  const result = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) result.push(...(await filesUnder(path.join(directory, entry.name))))
    else result.push(path.join(directory, entry.name))
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
