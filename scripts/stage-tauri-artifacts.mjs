import { copyFile, mkdir, readFile, readdir, rm } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const bundleDir = path.resolve(
  root,
  process.env.PISPER_TAURI_BUNDLE_DIR || path.join('src-tauri', 'target', 'release', 'bundle'),
)
const stageRoot = path.resolve(
  root,
  process.env.PISPER_TAURI_STAGE_DIR || path.join('release', 'tauri-artifacts'),
)
const requireSignature = process.argv.includes('--require-signature')
const desktopPackage = JSON.parse(
  await readFile(path.join(root, 'src-tauri', 'desktop-package.json'), 'utf8'),
)
const version = desktopPackage.version

const platform =
  process.platform === 'win32'
    ? 'windows'
    : process.platform === 'darwin'
      ? 'darwin'
      : process.platform
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const target = `${platform}-${arch}`
const stageDir = path.join(stageRoot, target)

async function filesUnder(directory) {
  const result = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const fullPath = path.join(directory, entry.name)
    if (entry.isDirectory()) result.push(...(await filesUnder(fullPath)))
    else result.push(fullPath)
  }
  return result
}

function exactlyOne(files, predicate, label) {
  const matches = files.filter(predicate)
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${label} under ${bundleDir}; found ${matches.length}.`)
  }
  return matches[0]
}

function normalizedName(suffix) {
  return `Pisper_${version}_${platform}_${arch}${suffix}`
}

await rm(stageDir, { recursive: true, force: true })
await mkdir(stageDir, { recursive: true })
const files = await filesUnder(bundleDir)
const staged = []

async function stage(source, destinationName) {
  const destination = path.join(stageDir, destinationName)
  await copyFile(source, destination)
  staged.push(destination)
  return destination
}

async function stageUpdater(source, destinationName) {
  await stage(source, destinationName)
  const signaturePath = `${source}.sig`
  if (files.includes(signaturePath)) {
    await stage(signaturePath, `${destinationName}.sig`)
  } else if (requireSignature) {
    throw new Error(`Missing updater signature: ${signaturePath}`)
  }
}

if (platform === 'windows') {
  const installer = exactlyOne(
    files,
    (file) => file.endsWith('-setup.exe') && !file.endsWith('.sig'),
    'Windows NSIS installer',
  )
  await stageUpdater(installer, normalizedName('-setup.exe'))
} else if (platform === 'darwin') {
  const dmg = exactlyOne(files, (file) => file.endsWith('.dmg'), 'macOS DMG')
  await stage(dmg, normalizedName('.dmg'))
  const updater = files.find((file) => file.endsWith('.app.tar.gz') && !file.endsWith('.sig'))
  if (updater) await stageUpdater(updater, normalizedName('.app.tar.gz'))
  else if (requireSignature) throw new Error(`No macOS updater archive found under ${bundleDir}.`)
} else if (platform === 'linux') {
  const appImage = exactlyOne(
    files,
    (file) => file.endsWith('.AppImage') && !file.endsWith('.sig'),
    'Linux AppImage',
  )
  await stageUpdater(appImage, normalizedName('.AppImage'))
  const deb = exactlyOne(files, (file) => file.endsWith('.deb'), 'Linux DEB package')
  await stage(deb, normalizedName('.deb'))
} else {
  throw new Error(`Unsupported Tauri release platform: ${process.platform}`)
}

console.log(`Staged ${target} Tauri artifacts:`)
for (const file of staged) console.log(`- ${path.relative(root, file)}`)
