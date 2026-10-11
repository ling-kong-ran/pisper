// 对三种桌面分发执行相同的真实二进制验收，缺库或漏带工具时禁止交付。
import { spawnSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const [executable, directory] = process.argv.slice(2)
if (!executable || !directory) throw new Error('Expected runtime executable and resource directory')
const root = resolve(directory)
const manifest = JSON.parse(await readFile(join(root, 'speech-native/resources.json'), 'utf8'))
const result = spawnSync(resolve(executable), ['--pisper-speech-worker'], {
  encoding: 'utf8',
  timeout: 30_000,
  input:
    JSON.stringify({
      id: 'packaging-version',
      method: 'version',
      params: { nativeLibraryDir: join(root, 'speech-native') },
    }) +
    '\n' +
    JSON.stringify({ method: 'shutdown' }) +
    '\n',
})
if (result.error || result.status !== 0)
  throw new Error('Staged native speech worker could not load its resources', {
    cause: result.error,
  })
const version = JSON.parse(result.stdout)
if (
  version.id !== 'packaging-version' ||
  version.ok !== true ||
  version.result?.sherpa !== manifest.engine.sherpa ||
  version.result?.onnxruntime !== manifest.engine.onnxruntime
)
  throw new Error('Staged speech libraries do not match their pinned versions')
for (const name of ['rg', 'fd']) {
  const provenance = JSON.parse(
    await readFile(join(root, 'bin', `${name}-provenance.json`), 'utf8'),
  )
  const tool = spawnSync(
    join(root, 'bin', name + (process.platform === 'win32' ? '.exe' : '')),
    ['--version'],
    { encoding: 'utf8', timeout: 10_000 },
  )
  if (
    tool.error ||
    tool.status !== 0 ||
    !tool.stdout.split(/\s+/).includes(provenance.version.replace(/^v/, ''))
  )
    throw new Error(`Staged ${name} is unavailable or has the wrong version`)
}
console.log('Rust speech and offline search resources verified.')
