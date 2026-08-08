import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { packageNpm } from './package-npm.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const { manifest, result, stage } = await packageNpm()
const [sourceKey, packagedKey] = await Promise.all([
  readFile(join(root, 'src-tauri', 'updater.pubkey'), 'utf8'),
  readFile(join(stage, 'updater.pubkey'), 'utf8'),
])

if (manifest.name !== 'pisper') throw new Error('npm package name must be pisper.')
if (manifest.private === true) throw new Error('staged npm package must not be private.')
if (manifest.bin?.pisper !== 'bin/pisper.mjs' || Object.keys(manifest.bin || {}).length !== 1) {
  throw new Error('npm package must expose only the pisper command.')
}
if (manifest.publishConfig?.access !== 'public' || manifest.publishConfig?.provenance !== true) {
  throw new Error('npm package must require public provenance publication.')
}
if (sourceKey.trim() !== packagedKey.trim()) {
  throw new Error('npm package updater key does not match the desktop and TUI updater key.')
}
if (result.unpackedSize > 5 * 1024 * 1024) {
  throw new Error(`npm launcher package is unexpectedly large: ${result.unpackedSize} bytes.`)
}
console.log(
  `Validated pisper@${manifest.version}: ${result.entryCount} files, ${result.unpackedSize} unpacked bytes.`,
)
