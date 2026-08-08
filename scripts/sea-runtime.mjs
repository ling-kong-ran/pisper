import { lstat, mkdir, readdir, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'

export const SEA_RUNTIME_MANIFEST_SCHEMA = 'pisper.sea-runtime-size-manifest'
export const SEA_RUNTIME_MANIFEST_VERSION = 1
export const SEA_RUNTIME_BUDGET_BYTES = 120 * 1024 * 1024

const PI_CODING_AGENT = 'node_modules/@earendil-works/pi-coding-agent'
const PI_NESTED_NODE_MODULES = `${PI_CODING_AGENT}/node_modules`
const CLIPBOARD_SCOPE = `${PI_NESTED_NODE_MODULES}/@mariozechner`
const PI_TUI = `${PI_NESTED_NODE_MODULES}/@earendil-works/pi-tui`

const removableDirectoryNames = new Set([
  '.github',
  '__tests__',
  'docs',
  'example',
  'examples',
  'test',
  'tests',
])
const removableFileNames = new Set(['changelog', 'changelog.md', 'readme', 'readme.md'])
const declarationSuffixes = ['.d.cts', '.d.mts', '.d.ts']
const sourceOnlySuffixes = ['.cts', '.mts', '.ts', '.tsx']
const officeBrowserFiles = new Set([
  'officeparser.browser.d.ts',
  'officeparser.browser.iife.js',
  'officeparser.browser.mjs',
  'officeparser.browser.slim.d.ts',
  'officeparser.browser.slim.iife.js',
  'officeparser.browser.slim.mjs',
])
const retainedYargsLocales = new Set(['en.json', 'zh_CN.json'])

function runtimePath(runtimeDir, relativePath) {
  return join(runtimeDir, ...relativePath.split('/'))
}

function emptyStats() {
  return { bytes: 0, fileCount: 0, directoryCount: 0 }
}

function addDirectory(stats) {
  stats.directoryCount += 1
}

function addFile(stats, bytes) {
  stats.bytes += bytes
  stats.fileCount += 1
}

async function treeStats(path) {
  let info
  try {
    info = await lstat(path)
  } catch (error) {
    if (error?.code === 'ENOENT') return null
    throw error
  }
  if (!info.isDirectory()) {
    return { bytes: info.size, fileCount: 1, directoryCount: 0 }
  }

  const result = { bytes: 0, fileCount: 0, directoryCount: 1 }
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = await treeStats(join(path, entry.name))
    if (!child) continue
    result.bytes += child.bytes
    result.fileCount += child.fileCount
    result.directoryCount += child.directoryCount
  }
  return result
}

async function listTopLevelPackages(nodeModules) {
  let entries
  try {
    entries = await readdir(nodeModules, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return []
    throw error
  }

  const packages = []
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name === '.bin') continue
    if (!entry.name.startsWith('@')) {
      packages.push(entry.name)
      continue
    }
    for (const scoped of await readdir(join(nodeModules, entry.name), { withFileTypes: true })) {
      if (scoped.isDirectory()) packages.push(`${entry.name}/${scoped.name}`)
    }
  }
  return packages.sort()
}

function packageNameForSegments(segments) {
  if (segments[0] !== 'node_modules' || segments.length < 2) return null
  if (!segments[1].startsWith('@')) return segments[1]
  return segments.length >= 3 ? `${segments[1]}/${segments[2]}` : null
}

function sortableStats(entries) {
  return [...entries.entries()]
    .map(([path, stats]) => ({ path, ...stats }))
    .sort((left, right) => right.bytes - left.bytes || left.path.localeCompare(right.path))
}

export async function collectRuntimeSnapshot(runtimeDir) {
  const total = { bytes: 0, fileCount: 0, directoryCount: 1 }
  const topLevel = new Map()
  const packages = new Map()
  const nodeModules = join(runtimeDir, 'node_modules')

  for (const entry of await readdir(runtimeDir, { withFileTypes: true })) {
    topLevel.set(entry.name, emptyStats())
  }
  for (const packageName of await listTopLevelPackages(nodeModules)) {
    packages.set(packageName, emptyStats())
  }

  async function walk(directory, segments) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const entryPath = join(directory, entry.name)
      const nextSegments = [...segments, entry.name]
      const topStats = topLevel.get(nextSegments[0])
      const packageStats = packages.get(packageNameForSegments(nextSegments))

      if (entry.isDirectory()) {
        addDirectory(total)
        if (topStats) addDirectory(topStats)
        if (packageStats) addDirectory(packageStats)
        await walk(entryPath, nextSegments)
        continue
      }

      const info = await lstat(entryPath)
      addFile(total, info.size)
      if (topStats) addFile(topStats, info.size)
      if (packageStats) addFile(packageStats, info.size)
    }
  }

  await walk(runtimeDir, [])
  return {
    ...total,
    packageCount: packages.size,
    topLevel: sortableStats(topLevel),
    packages: sortableStats(packages),
  }
}

function createRemovalAudit() {
  return { bytes: 0, fileCount: 0, directoryCount: 0, rules: {} }
}

function recordRemoval(audit, rule, stats) {
  audit.bytes += stats.bytes
  audit.fileCount += stats.fileCount
  audit.directoryCount += stats.directoryCount
  audit.rules[rule] ||= { bytes: 0, fileCount: 0, directoryCount: 0 }
  audit.rules[rule].bytes += stats.bytes
  audit.rules[rule].fileCount += stats.fileCount
  audit.rules[rule].directoryCount += stats.directoryCount
}

async function removePath(path, rule, audit) {
  const stats = await treeStats(path)
  if (!stats) return false
  await rm(path, { recursive: true, force: true })
  recordRemoval(audit, rule, stats)
  return true
}

function removableFileRule(name) {
  const lowerName = name.toLowerCase()
  if (removableFileNames.has(lowerName)) return 'packageMetadata'
  if (declarationSuffixes.some((suffix) => lowerName.endsWith(suffix))) return 'declarations'
  if (lowerName.endsWith('.map')) return 'sourceMaps'
  if (lowerName.endsWith('.tsbuildinfo')) return 'buildMetadata'
  if (sourceOnlySuffixes.some((suffix) => lowerName.endsWith(suffix))) return 'sourceOnly'
  return null
}

async function prunePackageTree(directory, audit) {
  let entries
  try {
    entries = await readdir(directory, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }

  const directoryName = basename(directory)
  const protectsPackageChildren =
    directoryName === 'node_modules' ||
    (basename(dirname(directory)) === 'node_modules' && directoryName.startsWith('@'))
  for (const entry of entries) {
    const entryPath = join(directory, entry.name)
    if (entry.isDirectory()) {
      if (!protectsPackageChildren && removableDirectoryNames.has(entry.name.toLowerCase())) {
        await removePath(entryPath, 'developmentDirectories', audit)
      } else {
        await prunePackageTree(entryPath, audit)
      }
      continue
    }
    const rule = removableFileRule(entry.name)
    if (rule) await removePath(entryPath, rule, audit)
  }
}

export function detectLinuxLibc(report = process.report?.getReport?.()) {
  if (report?.header?.glibcVersionRuntime) return 'gnu'
  if (
    Array.isArray(report?.sharedObjects) &&
    report.sharedObjects.some((path) => /(?:libc\.musl-|ld-musl-)/.test(String(path)))
  ) {
    return 'musl'
  }
  return null
}

export function runtimeTarget({ platform = process.platform, arch = process.arch, report } = {}) {
  return {
    platform,
    arch,
    libc: platform === 'linux' ? detectLinuxLibc(report) : null,
  }
}

export function selectClipboardPackage({ platform, arch, libc = null }) {
  if (platform === 'darwin' && (arch === 'x64' || arch === 'arm64')) {
    return 'clipboard-darwin-universal'
  }
  if (platform === 'win32' && (arch === 'x64' || arch === 'arm64')) {
    return `clipboard-win32-${arch}-msvc`
  }
  if (platform !== 'linux') return null
  if (arch === 'riscv64' && libc === 'gnu') return 'clipboard-linux-riscv64-gnu'
  if ((arch === 'x64' || arch === 'arm64') && (libc === 'gnu' || libc === 'musl')) {
    return `clipboard-linux-${arch}-${libc}`
  }
  return null
}

export function selectPiTuiNativeFiles({ platform, arch }) {
  if (platform === 'linux') return []
  if (platform === 'win32' && (arch === 'x64' || arch === 'arm64')) {
    return [`native/win32/prebuilds/win32-${arch}/win32-console-mode.node`]
  }
  if (platform === 'darwin' && (arch === 'x64' || arch === 'arm64')) {
    return [`native/darwin/prebuilds/darwin-${arch}/darwin-modifiers.node`]
  }
  return null
}

async function pruneOfficeBrowserBundles(nodeModules, audit) {
  const dist = join(nodeModules, 'officeparser', 'dist')
  let entries
  try {
    entries = await readdir(dist, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }
  for (const entry of entries) {
    if (entry.isFile() && officeBrowserFiles.has(entry.name)) {
      await removePath(join(dist, entry.name), 'officeparserBrowserBundles', audit)
    }
  }
  await removePath(join(dist, 'sbom.cdx.json'), 'buildMetadata', audit)
}

async function pruneYargsLocales(nodeModules, audit) {
  const locales = join(nodeModules, 'yargs', 'locales')
  let entries
  try {
    entries = await readdir(locales, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }
  for (const entry of entries) {
    if (entry.isFile() && !retainedYargsLocales.has(entry.name)) {
      await removePath(join(locales, entry.name), 'unusedYargsLocales', audit)
    }
  }
}

async function pruneClipboardPackages(runtimeDir, target, audit) {
  const scope = runtimePath(runtimeDir, CLIPBOARD_SCOPE)
  const selectedPackage = selectClipboardPackage(target)
  if (!selectedPackage) return null

  let entries
  try {
    entries = await readdir(scope, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return selectedPackage
    throw error
  }
  for (const entry of entries) {
    if (
      entry.isDirectory() &&
      entry.name.startsWith('clipboard-') &&
      entry.name !== selectedPackage
    ) {
      await removePath(join(scope, entry.name), 'foreignClipboardNative', audit)
    }
  }
  return selectedPackage
}

async function listRelativeFiles(directory, suffix = '') {
  const files = []
  async function walk(current, segments) {
    let entries
    try {
      entries = await readdir(current, { withFileTypes: true })
    } catch (error) {
      if (error?.code === 'ENOENT') return
      throw error
    }
    for (const entry of entries) {
      const nextSegments = [...segments, entry.name]
      if (entry.isDirectory()) {
        await walk(join(current, entry.name), nextSegments)
      } else if (!suffix || entry.name.endsWith(suffix)) {
        files.push(nextSegments.join('/'))
      }
    }
  }
  await walk(directory, [])
  return files.sort()
}

async function prunePiTuiNative(runtimeDir, target, audit) {
  const selectedFiles = selectPiTuiNativeFiles(target)
  if (selectedFiles === null) return null
  const nativeRoot = runtimePath(runtimeDir, `${PI_TUI}/native`)
  const keep = new Set(selectedFiles.map((path) => path.replace(/^native\//, '')))
  for (const relativePath of await listRelativeFiles(nativeRoot)) {
    if (!keep.has(relativePath)) {
      await removePath(runtimePath(nativeRoot, relativePath), 'foreignPiTuiNative', audit)
    }
  }
  return selectedFiles
}

export async function pruneRuntime(runtimeDir, target = runtimeTarget()) {
  const nodeModules = join(runtimeDir, 'node_modules')
  const audit = createRemovalAudit()
  const explicitPaths = [
    ['tesseract.js'],
    ['tesseract.js-core'],
    ['zlibjs'],
    ['@napi-rs', 'canvas'],
    ['@earendil-works', 'pi-coding-agent', 'CHANGELOG.md'],
    ['@earendil-works', 'pi-coding-agent', 'containerization.md'],
    ['@larksuiteoapi', 'node-sdk', 'es'],
    ['@larksuiteoapi', 'node-sdk', 'types'],
    ['openai', 'src'],
    ['pdfjs-dist', 'image_decoders'],
    ['pdfjs-dist', 'web'],
    ['highlight.js', 'scss'],
    ['highlight.js', 'styles'],
  ]
  for (const parts of explicitPaths) {
    await removePath(join(nodeModules, ...parts), 'explicitUnusedClosure', audit)
  }

  const pdfBuild = join(nodeModules, 'pdfjs-dist', 'build')
  for (const name of [
    'pdf.min.mjs',
    'pdf.sandbox.mjs',
    'pdf.sandbox.min.mjs',
    'pdf.worker.mjs',
    'pdf.worker.min.mjs',
  ]) {
    await removePath(join(pdfBuild, name), 'unusedPdfjsBrowserRuntime', audit)
  }

  const canvasScope = join(nodeModules, '@napi-rs')
  let canvasEntries = []
  try {
    canvasEntries = await readdir(canvasScope, { withFileTypes: true })
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
  for (const entry of canvasEntries) {
    if (entry.isDirectory() && entry.name.startsWith('canvas-')) {
      await removePath(join(canvasScope, entry.name), 'unusedCanvasNative', audit)
    }
  }

  await pruneOfficeBrowserBundles(nodeModules, audit)
  await pruneYargsLocales(nodeModules, audit)
  const clipboardPackage = await pruneClipboardPackages(runtimeDir, target, audit)
  const piTuiNativeFiles = await prunePiTuiNative(runtimeDir, target, audit)
  await prunePackageTree(nodeModules, audit)

  return {
    audit,
    nativeSelection: { clipboardPackage, piTuiNativeFiles },
  }
}

export function criticalRuntimeEntries(nativeSelection = {}) {
  const entries = [
    ['app', 'runtime/sidecar.mjs'],
    ['skills', `${PI_CODING_AGENT}/dist/core/skills.js`],
    ['tools', `${PI_CODING_AGENT}/dist/core/tools/index.js`],
    ['extensions', `${PI_CODING_AGENT}/dist/core/extensions/index.js`],
    ['tui', `${PI_TUI}/dist/index.js`],
    ['officeparser', 'node_modules/officeparser/dist/index.mjs'],
    ['officeparser', 'node_modules/officeparser/dist/index.js'],
    ['officeparser', 'node_modules/officeparser/dist/OfficeParser.js'],
    ['officeparser', 'node_modules/officeparser/dist/OfficeGenerator.js'],
    ['officeparser', 'node_modules/officeparser/dist/parsers/WordParser.js'],
    ['officeparser', 'node_modules/officeparser/dist/parsers/PdfParser.js'],
    ['officeparser', 'node_modules/officeparser/dist/generators/MarkdownGenerator.js'],
    ['officeparser', 'node_modules/fflate/lib/node.cjs'],
    ['officeparser', 'node_modules/fflate/esm/index.mjs'],
    ['officeparser', 'node_modules/file-type/source/index.js'],
    ['officeparser', 'node_modules/@xmldom/xmldom/lib/index.js'],
    ['pdfjs', 'node_modules/pdfjs-dist/legacy/build/pdf.mjs'],
    ['pdfjs', 'node_modules/pdfjs-dist/legacy/build/pdf.worker.mjs'],
    ['mcp', 'node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js'],
    ['mcp', 'node_modules/@modelcontextprotocol/sdk/dist/esm/client/sse.js'],
    ['mcp', 'node_modules/@modelcontextprotocol/sdk/dist/esm/client/stdio.js'],
    ['mcp', 'node_modules/@modelcontextprotocol/sdk/dist/esm/client/streamableHttp.js'],
    ['playwright', 'node_modules/playwright-core/index.mjs'],
    ['locales', 'node_modules/yargs/locales/en.json'],
    ['locales', 'node_modules/zod/v4/locales/index.js'],
    ['locales', 'node_modules/typebox/build/index.mjs'],
    ['native', `${CLIPBOARD_SCOPE}/clipboard/index.js`],
  ]

  if (nativeSelection.clipboardPackage) {
    const packageName = nativeSelection.clipboardPackage
    const packageRoot = `${CLIPBOARD_SCOPE}/${packageName}`
    entries.push(['native', `${packageRoot}/package.json`])
    entries.push([
      'native',
      `${packageRoot}/clipboard.${packageName.slice('clipboard-'.length)}.node`,
    ])
  }
  for (const relativePath of nativeSelection.piTuiNativeFiles || []) {
    entries.push(['native', `${PI_TUI}/${relativePath}`])
  }
  return entries.map(([kind, path]) => ({ kind, path }))
}

export async function inspectCriticalFiles(runtimeDir, entries = criticalRuntimeEntries()) {
  const inspected = []
  for (const entry of entries) {
    const stats = await treeStats(runtimePath(runtimeDir, entry.path))
    inspected.push({
      ...entry,
      exists: stats !== null,
      bytes: stats?.bytes || 0,
      fileCount: stats?.fileCount || 0,
    })
  }
  return inspected
}

export async function collectNativeState(runtimeDir, nativeSelection) {
  const clipboardScope = runtimePath(runtimeDir, CLIPBOARD_SCOPE)
  let clipboardPackages = []
  try {
    clipboardPackages = (await readdir(clipboardScope, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory() && entry.name.startsWith('clipboard-'))
      .map((entry) => entry.name)
      .sort()
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  const piTuiNativeFiles = await listRelativeFiles(
    runtimePath(runtimeDir, `${PI_TUI}/native`),
    '.node',
  )
  const expectedClipboard = nativeSelection.clipboardPackage
  const selectedPiTui = nativeSelection.piTuiNativeFiles
  const expectedPiTui = selectedPiTui?.map((path) => path.replace(/^native\//, ''))
  const clipboardPass =
    expectedClipboard === null ||
    (clipboardPackages.length === 1 && clipboardPackages[0] === expectedClipboard)
  const piTuiPass =
    selectedPiTui === null ||
    (piTuiNativeFiles.length === expectedPiTui.length &&
      piTuiNativeFiles.every((path, index) => path === [...expectedPiTui].sort()[index]))

  return {
    selection: nativeSelection,
    retainedClipboardPackages: clipboardPackages,
    retainedPiTuiNativeFiles: piTuiNativeFiles,
    clipboardPass,
    piTuiPass,
    pass: clipboardPass && piTuiPass,
  }
}

function reduction(before, after) {
  const bytes = before.bytes - after.bytes
  return {
    bytes,
    fileCount: before.fileCount - after.fileCount,
    percent: before.bytes ? Number(((bytes / before.bytes) * 100).toFixed(2)) : 0,
  }
}

export function createSizeManifest({
  appVersion,
  nodeVersion = process.version,
  target,
  beforePrune,
  afterPrune,
  pruning,
  criticalFiles,
  native,
  executableBytes = null,
  budgetBytes = SEA_RUNTIME_BUDGET_BYTES,
  generatedAt = new Date().toISOString(),
}) {
  const budgetPass = afterPrune.bytes <= budgetBytes
  const criticalFilesPass = criticalFiles.every((entry) => entry.exists)
  const pass = budgetPass && criticalFilesPass && native.pass
  return {
    schema: SEA_RUNTIME_MANIFEST_SCHEMA,
    version: SEA_RUNTIME_MANIFEST_VERSION,
    generatedAt,
    appVersion,
    nodeVersion,
    platform: target.platform,
    arch: target.arch,
    libc: target.libc,
    runtime: {
      beforePrune,
      afterPrune,
      reduction: reduction(beforePrune, afterPrune),
    },
    pruning,
    criticalFiles,
    native,
    budget: {
      runtimeBytes: budgetBytes,
      runtimeMiB: budgetBytes / 1024 / 1024,
      actualBytes: afterPrune.bytes,
      pass: budgetPass,
    },
    sidecarExecutableBytes: executableBytes,
    validation: {
      criticalFilesPass,
      nativePass: native.pass,
    },
    pass,
  }
}

export function finalizeSizeManifest(manifest, executableBytes) {
  return { ...manifest, sidecarExecutableBytes: executableBytes }
}

export function assertSizeManifest(manifest, { requireExecutable = false } = {}) {
  const failures = []
  if (!manifest.budget.pass) {
    failures.push(
      `runtime ${manifest.budget.actualBytes} bytes exceeds ${manifest.budget.runtimeBytes} byte budget`,
    )
  }
  const missing = manifest.criticalFiles.filter((entry) => !entry.exists).map((entry) => entry.path)
  if (missing.length) failures.push(`missing critical runtime files: ${missing.join(', ')}`)
  if (!manifest.native.pass)
    failures.push('native package selection does not match the build target')
  if (requireExecutable && !Number.isSafeInteger(manifest.sidecarExecutableBytes)) {
    failures.push('sidecar executable size was not recorded')
  }
  if (failures.length) throw new Error(`SEA runtime audit failed: ${failures.join('; ')}`)
}

export async function writeSizeManifest(path, manifest) {
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
}
