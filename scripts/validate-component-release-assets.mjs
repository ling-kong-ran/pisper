import { readdir } from 'node:fs/promises'
import { basename, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { assertReleaseComponent, releaseTag, releaseVersionFromTag } from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const component = assertReleaseComponent(process.argv[2])
const tag = String(process.argv[3] || '').trim()
const directory = resolve(root, process.argv[4] || 'artifacts')
if (component === 'desktop') {
  throw new Error('Use scripts/validate-tauri-release-assets.mjs for desktop releases.')
}
const version = releaseVersionFromTag(component, tag)
if (!version || releaseTag(component, version) !== tag) {
  throw new Error(`Invalid ${component} release tag: ${tag}`)
}

async function filesUnder(path) {
  const files = []
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = resolve(path, entry.name)
    if (entry.isDirectory()) files.push(...(await filesUnder(child)))
    else files.push(child)
  }
  return files
}

const label = component === 'tui' ? 'TUI' : 'Server'
const expected = [
  `Pisper_${label}_${version}_darwin_aarch64.tar.gz`,
  `Pisper_${label}_${version}_darwin_x86_64.tar.gz`,
  `Pisper_${label}_${version}_linux_x86_64.tar.gz`,
  `Pisper_${label}_${version}_windows_x86_64.tar.gz`,
].sort()
const actual = (await filesUnder(directory)).map((file) => basename(file)).sort()
const missing = expected.filter((name) => !actual.includes(name))
const unexpected = actual.filter((name) => !expected.includes(name))
if (missing.length || unexpected.length) {
  const details = []
  if (missing.length) details.push(`Missing release assets: ${missing.join(', ')}`)
  if (unexpected.length) details.push(`Unexpected release assets: ${unexpected.join(', ')}`)
  throw new Error(details.join('\n'))
}
console.log(`Validated ${component} release assets for ${tag}: ${actual.join(', ')}`)
