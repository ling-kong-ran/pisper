import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
const manifestPath = join(root, 'src-tui', 'Cargo.toml')
const manifest = await readFile(manifestPath, 'utf8')
const updated = manifest.replace(
  /(\[package\][\s\S]*?\nversion\s*=\s*")[^"]+("\s*\n)/,
  `$1${packageJson.version}$2`,
)
if (updated === manifest && !manifest.includes(`version = "${packageJson.version}"`)) {
  throw new Error('Unable to synchronize src-tui/Cargo.toml version.')
}
await writeFile(manifestPath, updated, 'utf8')
console.log(`Synchronized Pisper TUI version ${packageJson.version}.`)
