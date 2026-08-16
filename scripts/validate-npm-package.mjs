import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { npmPlatformOptionalDependencies } from '../packages/pisper/lib/npm-platform.mjs'
import { packageNpm } from './package-npm.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const { manifest, result, stage } = await packageNpm()
const [
  sourceKey,
  packagedKey,
  postinstall,
  webIndex,
  sourceChineseReadme,
  sourceEnglishReadme,
  packagedChineseReadme,
  packagedEnglishReadme,
] = await Promise.all([
  readFile(join(root, 'src-tauri', 'updater.pubkey'), 'utf8'),
  readFile(join(stage, 'updater.pubkey'), 'utf8'),
  readFile(join(stage, 'lib', 'postinstall.mjs'), 'utf8'),
  readFile(join(stage, 'web', 'index.html'), 'utf8'),
  readFile(join(root, 'src-tui', 'README.md'), 'utf8'),
  readFile(join(root, 'src-tui', 'README.en.md'), 'utf8'),
  readFile(join(stage, 'README.md'), 'utf8'),
  readFile(join(stage, 'README.en.md'), 'utf8'),
])

if (manifest.name !== 'pisper') throw new Error('npm package name must be pisper.')
if (manifest.private === true) throw new Error('staged npm package must not be private.')
if (manifest.bin?.pisper !== 'bin/pisper.mjs' || Object.keys(manifest.bin || {}).length !== 1) {
  throw new Error('npm package must expose only the pisper command.')
}
if (manifest.publishConfig?.access !== 'public' || manifest.publishConfig?.provenance !== true) {
  throw new Error('npm package must require public provenance publication.')
}
if (
  JSON.stringify(manifest.optionalDependencies) !==
  JSON.stringify(npmPlatformOptionalDependencies(manifest.version))
) {
  throw new Error(
    'npm package must select the exact platform bundles through optional dependencies.',
  )
}
if (manifest.scripts?.postinstall !== 'node lib/postinstall.mjs') {
  throw new Error('npm package must prepare its local platform bundle during postinstall.')
}
if (
  !postinstall.includes('ensurePisperInstallation') ||
  /\bfetch\(|github\.com/.test(postinstall)
) {
  throw new Error('npm postinstall must prepare only the local signed platform bundle.')
}
if (sourceKey.trim() !== packagedKey.trim()) {
  throw new Error('npm package updater key does not match the desktop and TUI updater key.')
}
if (!webIndex.includes('<div id="root"></div>')) {
  throw new Error('npm package does not contain the built Web frontend.')
}
if (
  sourceChineseReadme !== packagedChineseReadme ||
  sourceEnglishReadme !== packagedEnglishReadme
) {
  throw new Error('npm package README files must match the TUI guides.')
}
if (result.size > 8 * 1024 * 1024 || result.unpackedSize > 16 * 1024 * 1024) {
  throw new Error(
    `npm launcher and Web package is unexpectedly large: ${result.size}/${result.unpackedSize} bytes.`,
  )
}
console.log(
  `Validated pisper@${manifest.version}: ${result.entryCount} files, ${result.unpackedSize} unpacked bytes.`,
)
