import { execFileSync } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { npmPlatformOptionalDependencies } from '../packages/pisper/lib/npm-platform.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'packages', 'pisper')
const releaseRoot = join(root, 'release', 'npm')
const stage = join(releaseRoot, 'package', 'pisper')
const tarballs = join(releaseRoot, 'tarballs')

function npm(args) {
  const npmCli = String(process.env.npm_execpath || '').trim()
  const command = npmCli ? process.execPath : process.platform === 'win32' ? 'npm.cmd' : 'npm'
  const commandArgs = npmCli ? [npmCli, ...args] : args
  return execFileSync(command, commandArgs, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()
}

function version(value, label) {
  const normalized = String(value || '').trim()
  if (!/^\d+\.\d+\.\d+$/.test(normalized)) throw new Error(`${label} must use X.Y.Z format.`)
  return normalized
}

export async function packageNpm() {
  const manifest = JSON.parse(await readFile(join(source, 'package.json'), 'utf8'))
  manifest.version = version(process.env.PISPER_NPM_VERSION || manifest.version, 'npm version')
  manifest.pisper.tuiVersion = version(
    process.env.PISPER_NPM_TUI_VERSION || manifest.pisper.tuiVersion,
    'TUI version',
  )
  manifest.pisper.runtimeVersion = version(
    process.env.PISPER_NPM_RUNTIME_VERSION || manifest.pisper.runtimeVersion,
    'Runtime version',
  )
  delete manifest.private
  manifest.optionalDependencies = npmPlatformOptionalDependencies(manifest.version)
  manifest.publishConfig = { access: 'public', provenance: true }

  await rm(join(releaseRoot, 'package'), { recursive: true, force: true })
  await rm(tarballs, { recursive: true, force: true })
  await Promise.all([mkdir(stage, { recursive: true }), mkdir(tarballs, { recursive: true })])
  await Promise.all([
    cp(join(source, 'bin'), join(stage, 'bin'), { recursive: true }),
    cp(join(source, 'lib'), join(stage, 'lib'), { recursive: true }),
    cp(join(root, 'dist'), join(stage, 'web'), { recursive: true }),
    copyFile(join(source, 'updater.pubkey'), join(stage, 'updater.pubkey')),
    copyFile(join(root, 'src-tui', 'README.md'), join(stage, 'README.md')),
    copyFile(join(root, 'src-tui', 'README.en.md'), join(stage, 'README.en.md')),
    copyFile(join(root, 'LICENSE'), join(stage, 'LICENSE')),
    writeFile(join(stage, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8'),
  ])
  if (process.platform !== 'win32') await chmod(join(stage, 'bin', 'pisper.mjs'), 0o755)

  const result = JSON.parse(npm(['pack', stage, '--pack-destination', tarballs, '--json']))[0]
  const expected = [
    'LICENSE',
    'README.en.md',
    'README.md',
    'bin/pisper.mjs',
    'lib/install.mjs',
    'lib/npm-platform.mjs',
    'lib/npm-update.mjs',
    'lib/platform.mjs',
    'lib/postinstall.mjs',
    'package.json',
    'updater.pubkey',
    'web/index.html',
  ]
  const actual = result.files.map(({ path }) => path).sort()
  const missing = expected.filter((path) => !actual.includes(path))
  const unexpected = actual.filter((path) => !expected.includes(path) && !path.startsWith('web/'))
  if (missing.length || unexpected.length) {
    throw new Error(
      [
        missing.length ? `Missing npm package files: ${missing.join(', ')}` : '',
        unexpected.length ? `Unexpected npm package files: ${unexpected.join(', ')}` : '',
      ]
        .filter(Boolean)
        .join('\n'),
    )
  }
  if (result.name !== 'pisper' || result.version !== manifest.version) {
    throw new Error(`npm pack returned unexpected identity: ${result.name}@${result.version}`)
  }

  const tarball = join(tarballs, basename(result.filename))
  await writeFile(
    join(releaseRoot, 'pack-result.json'),
    `${JSON.stringify({ ...result, tarball }, null, 2)}\n`,
    'utf8',
  )
  console.log(`Packed pisper@${manifest.version}: ${tarball}`)
  return { manifest, result, stage, tarball }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await packageNpm()
}
