import { build } from 'esbuild'
import { cp, mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, join, relative, resolve } from 'node:path'

export const RUNTIME_BUNDLE_SCHEMA = 'pisper.runtime-bundle-manifest'
export const RUNTIME_BUNDLE_VERSION = 1

// 这些包依赖包目录、动态子模块、原生文件或用户扩展解析，不能安全内联到单个 ESM bundle。
export const RUNTIME_EXTERNAL_PACKAGES = Object.freeze([
  '@earendil-works/pi-coding-agent',
  '@homebridge/ciao',
  '@larksuiteoapi/node-sdk',
  '@modelcontextprotocol/sdk',
  'officeparser',
  'playwright-core',
])

const BUILD_EXTERNAL_PACKAGES = [...RUNTIME_EXTERNAL_PACKAGES, 'vite']
const ENTRY_POINTS = Object.freeze({
  sidecar: 'runtime/sidecar.mjs',
  'mobile-embedded': 'runtime/mobile-embedded.mjs',
})

function posixPath(path) {
  return path.replaceAll('\\', '/')
}

async function collectFiles(directory, root = directory) {
  const files = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      files.push(...(await collectFiles(path, root)))
    } else if (entry.isFile()) {
      files.push({ path: posixPath(relative(root, path)), bytes: (await stat(path)).size })
    }
  }
  return files.sort((left, right) => left.path.localeCompare(right.path))
}

function retainedDependencies(packageJson) {
  const dependencies = packageJson.dependencies || {}
  return Object.fromEntries(
    RUNTIME_EXTERNAL_PACKAGES.map((name) => {
      const version = dependencies[name]
      if (!version) {
        throw new Error(`Runtime external package is not a production dependency: ${name}`)
      }
      return [name, version]
    }),
  )
}

function packageRootForInput(runtimeDir, inputPath) {
  let current = dirname(resolve(runtimeDir, inputPath))
  while (current !== runtimeDir) {
    const parent = dirname(current)
    if (basename(parent) === 'node_modules') return current
    const grandparent = dirname(parent)
    if (basename(parent).startsWith('@') && basename(grandparent) === 'node_modules') {
      return current
    }
    if (parent === current) break
    current = parent
  }
  return null
}

async function writeBundledLicenses(runtimeDir, inputPaths) {
  const packageRoots = new Set(
    inputPaths.map((path) => packageRootForInput(runtimeDir, path)).filter(Boolean),
  )
  const blocks = []
  for (const packageRoot of [...packageRoots].sort()) {
    const packageJson = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))
    const entries = await readdir(packageRoot, { withFileTypes: true })
    const licenseFiles = entries
      .filter(
        (entry) =>
          entry.isFile() && /^(?:license|licence|copying|notice)(?:[._-].*)?$/i.test(entry.name),
      )
      .map((entry) => entry.name)
      .sort()
    const contents = await Promise.all(
      licenseFiles.map(
        async (name) => `${name}:\n${await readFile(join(packageRoot, name), 'utf8')}`,
      ),
    )
    const fallback = packageJson.license ? [`SPDX license: ${packageJson.license}`] : []
    blocks.push(
      `================================================================================\n${packageJson.name}@${packageJson.version}\n================================================================================\n${[
        ...contents,
        ...(!contents.length ? fallback : []),
      ].join('\n\n')}`,
    )
  }
  await writeFile(
    join(runtimeDir, 'THIRD_PARTY_LICENSES.txt'),
    `Third-party licenses for bundled Pisper Runtime dependencies.\n\n${blocks.join('\n\n')}\n`,
    'utf8',
  )
}

export async function bundleRuntime({ runtimeDir }) {
  runtimeDir = resolve(runtimeDir)
  const sourceRuntime = join(runtimeDir, 'runtime')
  const sourceShared = join(runtimeDir, 'shared')
  const outputRoot = join(runtimeDir, '.pisper-runtime-bundle')
  const outputRuntime = join(outputRoot, 'runtime')
  const packagePath = join(runtimeDir, 'package.json')
  const packageJson = JSON.parse(await readFile(packagePath, 'utf8'))
  const dependencies = retainedDependencies(packageJson)

  await rm(outputRoot, { recursive: true, force: true })
  await mkdir(outputRuntime, { recursive: true })
  const result = await build({
    absWorkingDir: runtimeDir,
    entryPoints: ENTRY_POINTS,
    outdir: outputRuntime,
    bundle: true,
    splitting: true,
    format: 'esm',
    platform: 'node',
    target: 'node20',
    conditions: ['node', 'import'],
    outExtension: { '.js': '.mjs' },
    entryNames: '[name]',
    chunkNames: 'chunks/[name]-[hash]',
    external: BUILD_EXTERNAL_PACKAGES,
    // ESM chunk 需要真实 Node require，供已打包 CommonJS 依赖动态加载内建模块。
    banner: {
      js: "import { createRequire as __pisperCreateRequire } from 'node:module'\nconst require = __pisperCreateRequire(import.meta.url)",
    },
    metafile: true,
    treeShaking: true,
    legalComments: 'eof',
    minifySyntax: true,
    logLevel: 'silent',
  })

  // 独立 Worker 不能并入主进程 bundle；保留稳定目录也让 import.meta.url 在源码和 chunks 中一致解析。
  await Promise.all([
    mkdir(join(outputRuntime, 'plugins'), { recursive: true }),
    mkdir(join(outputRuntime, 'workers'), { recursive: true }),
  ])
  await Promise.all([
    cp(
      join(sourceRuntime, 'plugins', 'local-plugin-worker.mjs'),
      join(outputRuntime, 'plugins', 'local-plugin-worker.mjs'),
    ),
    cp(
      join(sourceRuntime, 'workers', 'team-workflow-worker.mjs'),
      join(outputRuntime, 'workers', 'team-workflow-worker.mjs'),
    ),
  ])

  await rm(sourceRuntime, { recursive: true, force: true })
  await rename(outputRuntime, sourceRuntime)
  await rm(outputRoot, { recursive: true, force: true })
  await rm(sourceShared, { recursive: true, force: true })

  packageJson.dependencies = dependencies
  await writeFile(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, 'utf8')
  await writeBundledLicenses(runtimeDir, Object.keys(result.metafile.inputs))

  const files = await collectFiles(sourceRuntime)
  const manifest = {
    schema: RUNTIME_BUNDLE_SCHEMA,
    version: RUNTIME_BUNDLE_VERSION,
    entries: Object.keys(ENTRY_POINTS).map((name) => `runtime/${name}.mjs`),
    externalPackages: [...RUNTIME_EXTERNAL_PACKAGES],
    inputFileCount: Object.keys(result.metafile.inputs).length,
    inputBytes: Object.values(result.metafile.inputs).reduce((sum, input) => sum + input.bytes, 0),
    outputFileCount: files.length,
    outputBytes: files.reduce((sum, file) => sum + file.bytes, 0),
    licenseFile: 'THIRD_PARTY_LICENSES.txt',
    files: files.map((file) => ({ path: `runtime/${file.path}`, bytes: file.bytes })),
  }
  await writeFile(
    join(runtimeDir, 'runtime-bundle-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
    'utf8',
  )
  return manifest
}
