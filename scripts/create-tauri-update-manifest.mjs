import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const tag = String(args.find((value) => /^v/.test(value)) || '').trim()
const requireAll = args.includes('--require-all')
const outputIndex = args.indexOf('--output')
const artifactsArg = args.find(
  (value, index) => index > 0 && !value.startsWith('--') && args[index - 1] !== '--output',
)
const artifactsDir = path.resolve(root, artifactsArg || path.join('release', 'tauri-artifacts'))
const output = path.resolve(
  root,
  outputIndex >= 0 && args[outputIndex + 1]
    ? args[outputIndex + 1]
    : path.join('src-tauri', 'target', 'release', 'bundle', 'latest.json'),
)

if (!/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(tag)) {
  throw new Error(
    'Usage: node scripts/create-tauri-update-manifest.mjs v<version> [artifacts-dir] [--require-all] [--output <path>]',
  )
}

async function filesUnder(directory) {
  const result = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const fullPath = path.join(directory, entry.name)
    if (entry.isDirectory()) result.push(...(await filesUnder(fullPath)))
    else result.push(fullPath)
  }
  return result
}

const version = tag.slice(1)
const desktopPackage = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'desktop-package.json'), 'utf8'),
)
const updaterPublicKey = (
  await readFile(path.join(root, 'src-tauri', 'updater.pubkey'), 'utf8')
).trim()
const updaterConfig = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'tauri.updater.conf.json'), 'utf8'),
)
if (updaterConfig.plugins?.updater?.pubkey !== updaterPublicKey) {
  throw new Error('Tauri updater build configuration does not match src-tauri/updater.pubkey.')
}
if (desktopPackage.version !== version) {
  throw new Error(`Tag ${tag} does not match desktop version ${desktopPackage.version}.`)
}

const updaterSuffixes = {
  darwin: '.app.tar.gz',
  linux: '.AppImage',
  windows: '-setup.exe',
}
const expectedPlatforms = ['darwin-aarch64', 'darwin-x86_64', 'linux-x86_64', 'windows-x86_64']
const escapedVersion = version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
const artifactPattern = new RegExp(
  `^Pisper_${escapedVersion}_(darwin|linux|windows)_(aarch64|x86_64)(\\.app\\.tar\\.gz|\\.AppImage|-setup\\.exe)$`,
)
const files = await filesUnder(artifactsDir)
const platforms = {}

for (const file of files) {
  const match = path.basename(file).match(artifactPattern)
  if (!match) continue
  const [, platform, arch, suffix] = match
  if (updaterSuffixes[platform] !== suffix) continue
  const platformKey = `${platform}-${arch}`
  if (platforms[platformKey]) {
    throw new Error(`Duplicate updater artifact for ${platformKey} under ${artifactsDir}.`)
  }
  const signaturePath = `${file}.sig`
  const signature = (await readFile(signaturePath, 'utf8')).trim()
  if (!signature) throw new Error(`Updater signature is empty: ${signaturePath}`)
  const artifactName = path.basename(file)
  platforms[platformKey] = {
    signature,
    url: `https://github.com/ling-kong-ran/pisper/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(artifactName)}`,
  }
}

if (Object.keys(platforms).length === 0) {
  throw new Error(`No signed Tauri updater artifacts found under ${artifactsDir}.`)
}
if (requireAll) {
  const missing = expectedPlatforms.filter((platform) => !platforms[platform])
  if (missing.length > 0) throw new Error(`Missing Tauri updater platforms: ${missing.join(', ')}.`)
}

let notes = ''
try {
  notes = await readFile(path.join(root, 'release-body.md'), 'utf8')
} catch {
  // Local signed builds do not need generated release notes.
}

const manifest = {
  version,
  notes: notes.trim(),
  pub_date: new Date().toISOString(),
  platforms: Object.fromEntries(
    Object.entries(platforms).sort(([left], [right]) => left.localeCompare(right)),
  ),
}
await mkdir(path.dirname(output), { recursive: true })
await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`Tauri updater manifest: ${path.relative(root, output)}`)
