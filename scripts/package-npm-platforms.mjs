import { execFileSync } from 'node:child_process'
import { copyFile, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
  NPM_PLATFORM_TARGETS,
  npmPlatformAlias,
  npmPlatformVersion,
} from '../packages/pisper/lib/npm-platform.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'packages', 'pisper')
const releaseRoot = join(root, 'release', 'npm')
const artifacts = resolve(root, process.argv[2] || join(releaseRoot, 'component-artifacts'))
const stageRoot = join(releaseRoot, 'platform-packages')
const tarballs = join(releaseRoot, 'tarballs')
const MAX_COMPONENT_BYTES = 128 * 1024 * 1024
const MAX_PLATFORM_TARBALL_BYTES = 95 * 1024 * 1024

function npm(args) {
  const npmCli = String(process.env.npm_execpath || '').trim()
  const command = npmCli ? process.execPath : process.platform === 'win32' ? 'npm.cmd' : 'npm'
  const commandArgs = npmCli ? [npmCli, ...args] : args
  return execFileSync(command, commandArgs, { cwd: root, encoding: 'utf8', stdio: 'pipe' }).trim()
}

async function requireArtifact(name, maximum = MAX_COMPONENT_BYTES) {
  const path = join(artifacts, name)
  const bytes = (await stat(path)).size
  if (bytes <= 0 || bytes > maximum) {
    throw new Error(`Invalid npm component artifact size for ${name}: ${bytes} bytes.`)
  }
  return path
}

export async function packageNpmPlatforms() {
  const launcherManifest = JSON.parse(await readFile(join(source, 'package.json'), 'utf8'))
  const npmVersion = String(process.env.PISPER_NPM_VERSION || launcherManifest.version)
  const tuiVersion = String(
    process.env.PISPER_NPM_TUI_VERSION || launcherManifest.pisper.tuiVersion,
  )
  const runtimeVersion = String(
    process.env.PISPER_NPM_RUNTIME_VERSION || launcherManifest.pisper.runtimeVersion,
  )
  for (const [label, value] of [
    ['npm', npmVersion],
    ['TUI', tuiVersion],
    ['Runtime', runtimeVersion],
  ]) {
    if (!/^\d+\.\d+\.\d+$/.test(value)) throw new Error(`${label} version must use X.Y.Z.`)
  }

  await rm(stageRoot, { recursive: true, force: true })
  await Promise.all([mkdir(stageRoot, { recursive: true }), mkdir(tarballs, { recursive: true })])
  const packages = []

  for (const target of NPM_PLATFORM_TARGETS) {
    const alias = npmPlatformAlias(target.platform, target.arch)
    const version = npmPlatformVersion(npmVersion, target.platform, target.arch)
    const stage = join(stageRoot, alias)
    const tuiAsset = `Pisper_TUI_Component_${tuiVersion}_${target.release}.tar.gz`
    const runtimeAsset = `Pisper_Runtime_Node_${runtimeVersion}_${target.release}.tar.gz`
    const files = [
      [await requireArtifact(tuiAsset), join(stage, 'components', 'tui', tuiAsset)],
      [
        await requireArtifact(`${tuiAsset}.sig`, 4096),
        join(stage, 'components', 'tui', `${tuiAsset}.sig`),
      ],
      [await requireArtifact(runtimeAsset), join(stage, 'components', 'runtime', runtimeAsset)],
      [
        await requireArtifact(`${runtimeAsset}.sig`, 4096),
        join(stage, 'components', 'runtime', `${runtimeAsset}.sig`),
      ],
    ]
    await Promise.all(
      files.map(([, destination]) => mkdir(dirname(destination), { recursive: true })),
    )
    await Promise.all(files.map(([sourcePath, destination]) => copyFile(sourcePath, destination)))
    await Promise.all([
      copyFile(join(root, 'LICENSE'), join(stage, 'LICENSE')),
      copyFile(join(root, 'src-tui', 'README.en.md'), join(stage, 'README.md')),
    ])

    const manifest = {
      name: 'pisper',
      version,
      description: `Pisper signed TUI and Node Runtime bundle for ${target.slug}.`,
      license: 'MIT',
      author: launcherManifest.author,
      repository: launcherManifest.repository,
      os: [target.platform],
      cpu: [target.arch],
      files: ['components/', 'README.md', 'LICENSE'],
      publishConfig: { access: 'public', provenance: true },
      pisperBundle: {
        target: target.release,
        tuiVersion,
        runtimeVersion,
      },
    }
    await writeFile(join(stage, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
    const result = JSON.parse(npm(['pack', stage, '--pack-destination', tarballs, '--json']))[0]
    if (result.name !== 'pisper' || result.version !== version) {
      throw new Error(
        `npm pack returned unexpected platform identity: ${result.name}@${result.version}`,
      )
    }
    if (result.size > MAX_PLATFORM_TARBALL_BYTES) {
      throw new Error(`npm platform package ${version} is too large: ${result.size} bytes.`)
    }
    const tarball = join(tarballs, basename(result.filename))
    packages.push({ alias, version, target: target.release, tarball, size: result.size })
    console.log(`Packed ${alias} as pisper@${version}: ${tarball}`)
  }

  await writeFile(
    join(releaseRoot, 'platform-packages.json'),
    `${JSON.stringify(packages, null, 2)}\n`,
    'utf8',
  )
  return packages
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await packageNpmPlatforms()
}
