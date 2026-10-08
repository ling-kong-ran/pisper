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
// Rust 桌面流水线当前只在 Windows 产出 NSIS 安装包；其余平台恢复打包时
// 在这里补回对应资产（darwin .app.tar.gz/.dmg、linux .AppImage/.deb）。
const tauriAssets = [
  'latest.json',
  `Pisper_${version}_windows_x86_64-setup.exe`,
  `Pisper_${version}_windows_x86_64-setup.exe.sig`,
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
