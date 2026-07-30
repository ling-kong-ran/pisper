import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
const manifestPath = join(root, 'src-tui', 'Cargo.toml')
const lockPath = join(root, 'src-tui', 'Cargo.lock')
const manifest = await readFile(manifestPath, 'utf8')
const lock = await readFile(lockPath, 'utf8')
const updatedManifest = manifest.replace(
  /(\[package\][\s\S]*?\nversion\s*=\s*")[^"]+("\s*\n)/,
  `$1${packageJson.version}$2`,
)
const updatedLock = lock.replace(
  /(\[\[package\]\]\s*\nname\s*=\s*"pisper-tui"\s*\nversion\s*=\s*")[^"]+("\s*\n)/,
  `$1${packageJson.version}$2`,
)
if (updatedManifest === manifest && !manifest.includes(`version = "${packageJson.version}"`)) {
  throw new Error('Unable to synchronize src-tui/Cargo.toml version.')
}
if (updatedLock === lock && !lock.includes(`name = "pisper-tui"\nversion = "${packageJson.version}"`)) {
  throw new Error('Unable to synchronize src-tui/Cargo.lock version.')
}
await Promise.all([
  writeFile(manifestPath, updatedManifest, 'utf8'),
  writeFile(lockPath, updatedLock, 'utf8'),
])
console.log(`Synchronized Pisper TUI version ${packageJson.version}.`)
