import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const manifest = JSON.parse(
  await readFile(join(root, 'packages', 'pisper', 'package.json'), 'utf8'),
)
const repository = String(process.env.GITHUB_REPOSITORY || manifest.pisper.repository)
const token = String(process.env.GITHUB_TOKEN || '').trim()
const platforms = ['darwin_aarch64', 'darwin_x86_64', 'linux_x86_64', 'windows_x86_64']

async function validateRelease(component, version) {
  const tag = `${component}-v${version}`
  const response = await fetch(`https://api.github.com/repos/${repository}/releases/tags/${tag}`, {
    headers: {
      Accept: 'application/vnd.github+json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': `pisper/${manifest.version}`,
    },
  })
  if (!response.ok) throw new Error(`GitHub release ${tag} is unavailable: HTTP ${response.status}`)
  const release = await response.json()
  if (release.draft) throw new Error(`GitHub release ${tag} is still a draft.`)
  const label = component === 'tui' ? 'TUI_Component' : 'Runtime_Node'
  const names = new Set((release.assets || []).map(({ name }) => name))
  const expected = platforms.flatMap((platform) => {
    const archive = `Pisper_${label}_${version}_${platform}.tar.gz`
    return [archive, `${archive}.sig`]
  })
  const missing = expected.filter((name) => !names.has(name))
  if (missing.length)
    throw new Error(`${tag} is missing npm installer assets: ${missing.join(', ')}`)
  console.log(`Validated signed npm target ${tag}.`)
}

await validateRelease('tui', manifest.pisper.tuiVersion)
await validateRelease('runtime', manifest.pisper.runtimeVersion)
