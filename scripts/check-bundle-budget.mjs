import { readFile } from 'node:fs/promises'
import { gzipSync } from 'node:zlib'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const KIB = 1024

export const BUNDLE_BUDGETS = {
  totalCssGzip: 88 * KIB,
  entryFileGzip: 55 * KIB,
  entryStaticJsGzip: 275 * KIB,
  markdownSurfaceGzip: 330 * KIB,
  largestJsGzip: 245 * KIB,
  chunks: {
    'vendor-react': 100 * KIB,
    'vendor-state': 6 * KIB,
    'vendor-ui': 12 * KIB,
    'vendor-dockview': 90 * KIB,
    'vendor-xyflow': 65 * KIB,
    'vendor-motion': 50 * KIB,
    'vendor-markdown': 145 * KIB,
    'vendor-markdown-plugins': 105 * KIB,
    'vendor-shiki-runtime': 70 * KIB,
  },
}

const ROUTE_SOURCES = [
  'src/features/chat/ChatPage.tsx',
  'src/features/chat/ChatHistoryPage.tsx',
  'src/features/assets/AssetsPage.tsx',
  'src/features/channels/ChannelsPage.tsx',
  'src/features/schedules/SchedulesPage.tsx',
  'src/features/config/ConfigPage.tsx',
  'src/features/plugins/PluginsPage.tsx',
  'src/features/memory/MemoryPage.tsx',
  'src/features/workflows/PreviewPages.tsx',
  'src/features/skills/SkillsPage.tsx',
  'src/features/workflows/WorkflowsPage.tsx',
]

const REACT_BITS_DYNAMIC_SOURCES = [
  'src/components/react-bits/AnimatedList.tsx',
  'src/components/react-bits/ClickSpark.tsx',
  'src/components/react-bits/ShinyText.tsx',
  'src/components/react-bits/Threads.tsx',
  'src/features/chat/WelcomeEffects.tsx',
]

function formatSize(bytes) {
  return `${(bytes / 1000).toFixed(2)} kB`
}

function unique(values) {
  return [...new Set(values)]
}

function staticClosure(manifest, startKey) {
  const visited = new Set()
  const visit = (key) => {
    if (visited.has(key)) return
    visited.add(key)
    for (const dependency of manifest[key]?.imports || []) visit(dependency)
  }
  visit(startKey)
  return visited
}

function findStaticCycle(manifest) {
  const visited = new Set()
  const active = new Set()
  const path = []

  const visit = (key) => {
    if (active.has(key)) return [...path.slice(path.indexOf(key)), key]
    if (visited.has(key)) return null
    visited.add(key)
    active.add(key)
    path.push(key)
    for (const dependency of manifest[key]?.imports || []) {
      const cycle = visit(dependency)
      if (cycle) return cycle
    }
    path.pop()
    active.delete(key)
    return null
  }

  for (const key of Object.keys(manifest)) {
    const cycle = visit(key)
    if (cycle) return cycle
  }
  return null
}

export async function inspectBundle(distDirectory = resolve('dist')) {
  const manifestPath = resolve(distDirectory, '.vite/manifest.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const records = Object.entries(manifest)
  const fileMetrics = new Map()

  const measure = async (file) => {
    if (!fileMetrics.has(file)) {
      const contents = await readFile(resolve(distDirectory, file))
      fileMetrics.set(file, {
        raw: contents.byteLength,
        gzip: gzipSync(contents, { level: 9 }).byteLength,
      })
    }
    return fileMetrics.get(file)
  }

  for (const [, record] of records) {
    if (record.file.endsWith('.js') || record.file.endsWith('.css')) await measure(record.file)
    for (const cssFile of record.css || []) await measure(cssFile)
  }

  const keyByName = new Map(
    records.filter(([, record]) => record.name).map(([key, record]) => [record.name, key]),
  )
  const entryKey = records.find(([, record]) => record.isEntry)?.[0]
  if (!entryKey) throw new Error('Bundle manifest does not contain an application entry')

  const entryClosure = staticClosure(manifest, entryKey)
  const initialFiles = unique(
    [...entryClosure].map((key) => manifest[key]?.file).filter((file) => file?.endsWith('.js')),
  )
  const cssFiles = unique(
    records
      .flatMap(([, record]) => [record.file, ...(record.css || [])])
      .filter((file) => file.endsWith('.css')),
  )
  const jsFiles = unique(
    records.map(([, record]) => record.file).filter((file) => file.endsWith('.js')),
  )
  const developmentJsxFiles = []
  for (const file of jsFiles) {
    if (file.includes('vendor-react')) continue
    const contents = await readFile(resolve(distDirectory, file), 'utf8')
    if (/\.jsxDEV\)\(/.test(contents)) developmentJsxFiles.push(file)
  }
  const initialJsGzip = initialFiles.reduce((total, file) => total + fileMetrics.get(file).gzip, 0)
  const totalCssGzip = cssFiles.reduce((total, file) => total + fileMetrics.get(file).gzip, 0)
  const largestJs = jsFiles
    .map((file) => ({ file, ...fileMetrics.get(file) }))
    .sort((left, right) => right.gzip - left.gzip)[0]
  const markdownKey = keyByName.get('MarkdownMessage')
  const markdownClosure = markdownKey ? staticClosure(manifest, markdownKey) : new Set()
  const markdownSurfaceFiles = unique(
    [...markdownClosure]
      .filter((key) => !entryClosure.has(key))
      .map((key) => manifest[key]?.file)
      .filter((file) => file?.endsWith('.js')),
  )
  const markdownSurfaceGzip = markdownSurfaceFiles.reduce(
    (total, file) => total + fileMetrics.get(file).gzip,
    0,
  )

  return {
    cssFiles,
    developmentJsxFiles,
    entryClosure,
    entryKey,
    fileMetrics,
    initialJsGzip,
    keyByName,
    largestJs,
    manifest,
    manifestPath,
    markdownSurfaceGzip,
    records,
    totalCssGzip,
  }
}

export function validateBundle(report, budgets = BUNDLE_BUDGETS) {
  const failures = []
  const check = (label, actual, limit) => {
    if (actual > limit) failures.push(`${label}: ${formatSize(actual)} > ${formatSize(limit)}`)
  }

  const entry = report.manifest[report.entryKey]
  if (report.developmentJsxFiles.length) {
    failures.push(
      `development JSX runtime emitted in production: ${report.developmentJsxFiles.join(', ')}`,
    )
  }
  check('total CSS gzip', report.totalCssGzip, budgets.totalCssGzip)
  check('entry file gzip', report.fileMetrics.get(entry.file).gzip, budgets.entryFileGzip)
  check('entry static JS gzip', report.initialJsGzip, budgets.entryStaticJsGzip)
  check('Markdown surface gzip', report.markdownSurfaceGzip, budgets.markdownSurfaceGzip)
  check('largest JS chunk gzip', report.largestJs.gzip, budgets.largestJsGzip)

  for (const [name, limit] of Object.entries(budgets.chunks)) {
    const key = report.keyByName.get(name)
    if (!key) {
      failures.push(`missing stable chunk: ${name}`)
      continue
    }
    check(`${name} gzip`, report.fileMetrics.get(report.manifest[key].file).gzip, limit)
  }

  const cycle = findStaticCycle(report.manifest)
  if (cycle) failures.push(`static chunk cycle: ${cycle.join(' -> ')}`)

  const eagerVendorNames = new Set(
    [...report.entryClosure].map((key) => report.manifest[key]?.name).filter(Boolean),
  )
  for (const name of [
    'vendor-dockview',
    'vendor-markdown',
    'vendor-markdown-plugins',
    'vendor-motion',
    'vendor-shiki-runtime',
    'vendor-xyflow',
  ]) {
    if (eagerVendorNames.has(name)) failures.push(`route-only vendor is eager: ${name}`)
  }

  for (const source of ROUTE_SOURCES) {
    if (!report.manifest[source]?.isDynamicEntry) failures.push(`route is not lazy: ${source}`)
  }

  const dynamicShiki = (prefix) =>
    report.records.filter(([key, record]) => key.startsWith(prefix) && record.isDynamicEntry).length
  if (dynamicShiki('node_modules/@shikijs/langs/dist/') < 40)
    failures.push('Shiki grammar modules are not preserved as dynamic entries')
  if (dynamicShiki('node_modules/@shikijs/themes/dist/') < 20)
    failures.push('Shiki theme modules are not preserved as dynamic entries')
  if (!report.manifest['node_modules/shiki/dist/wasm.mjs']?.isDynamicEntry)
    failures.push('Shiki wasm is not preserved as a dynamic entry')

  const entryCss = new Set(entry.css || [])
  if ([...entryCss].some((file) => file.includes('react-bits')))
    failures.push('React Bits CSS is owned by the application entry')
  // 多个页面共用 React Bits 组件时，CSS 会随共享 chunk 一起打包；
  // 只要消费方的静态依赖闭包里能加载到 react-bits CSS 即视为归属正确。
  const transitiveCss = (key) => {
    const visited = new Set()
    const css = new Set()
    const walk = (current) => {
      if (visited.has(current)) return
      visited.add(current)
      const record = report.manifest[current]
      if (!record) return
      for (const file of record.css || []) css.add(file)
      for (const imported of record.imports || []) walk(imported)
    }
    walk(key)
    return css
  }
  for (const source of [...REACT_BITS_DYNAMIC_SOURCES, 'src/features/chat/ChatHistoryPage.tsx']) {
    const css = transitiveCss(source)
    if (![...css].some((file) => file.includes('react-bits')))
      failures.push(`React Bits CSS is not attached to its consumer: ${source}`)
  }

  return failures
}

export async function auditBundle(distDirectory = resolve('dist')) {
  const report = await inspectBundle(distDirectory)
  const failures = validateBundle(report)
  const rows = [
    ['entry', report.entryKey],
    ['ChatPage', 'src/features/chat/ChatPage.tsx'],
    ['MarkdownMessage', report.keyByName.get('MarkdownMessage')],
    ...Object.keys(BUNDLE_BUDGETS.chunks).map((name) => [name, report.keyByName.get(name)]),
    ['Shiki wasm', 'node_modules/shiki/dist/wasm.mjs'],
  ]

  console.log(`Bundle audit: ${report.manifestPath}`)
  for (const [label, key] of rows) {
    const file = report.manifest[key]?.file
    const size = file && report.fileMetrics.get(file)
    if (size) console.log(`  ${label}: ${formatSize(size.raw)} / ${formatSize(size.gzip)} gzip`)
  }
  console.log(`  entry static JS: ${formatSize(report.initialJsGzip)} gzip`)
  console.log(`  Markdown surface: ${formatSize(report.markdownSurfaceGzip)} gzip`)
  console.log(`  total CSS: ${formatSize(report.totalCssGzip)} gzip`)

  if (failures.length) throw new Error(`Bundle budget failed:\n- ${failures.join('\n- ')}`)
  console.log('Bundle budget passed')
  return report
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (isMain) await auditBundle()
