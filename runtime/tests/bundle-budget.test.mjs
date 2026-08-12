import assert from 'node:assert/strict'
import test from 'node:test'
import { BUNDLE_BUDGETS, validateBundle } from '../../scripts/check-bundle-budget.mjs'

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

const REACT_BITS_SOURCES = [
  'src/components/react-bits/AnimatedList.tsx',
  'src/components/react-bits/ClickSpark.tsx',
  'src/components/react-bits/ShinyText.tsx',
  'src/components/react-bits/Threads.tsx',
  'src/features/chat/WelcomeEffects.tsx',
]

function passingReport() {
  const manifest = {
    'index.html': {
      file: 'assets/index.js',
      name: 'index',
      isEntry: true,
      imports: [],
      css: ['assets/index.css'],
    },
    MarkdownMessage: {
      file: 'assets/MarkdownMessage.js',
      name: 'MarkdownMessage',
      imports: [],
    },
  }
  for (const name of Object.keys(BUNDLE_BUDGETS.chunks)) {
    manifest[`_${name}`] = { file: `assets/${name}.js`, name, imports: [] }
  }
  for (const source of ROUTE_SOURCES) {
    manifest[source] = { file: `assets/${source.split('/').at(-1)}.js`, isDynamicEntry: true }
  }
  manifest['src/features/chat/ChatHistoryPage.tsx'].css = ['assets/react-bits.css']
  for (const source of REACT_BITS_SOURCES) {
    manifest[source] = {
      file: `assets/${source.split('/').at(-1)}.js`,
      isDynamicEntry: true,
      css: ['assets/react-bits.css'],
    }
  }
  for (let index = 0; index < 40; index += 1) {
    manifest[`node_modules/@shikijs/langs/dist/lang-${index}.mjs`] = {
      file: `assets/lang-${index}.js`,
      isDynamicEntry: true,
    }
  }
  for (let index = 0; index < 20; index += 1) {
    manifest[`node_modules/@shikijs/themes/dist/theme-${index}.mjs`] = {
      file: `assets/theme-${index}.js`,
      isDynamicEntry: true,
    }
  }
  manifest['node_modules/shiki/dist/wasm.mjs'] = {
    file: 'assets/wasm.js',
    isDynamicEntry: true,
  }

  const fileMetrics = new Map()
  for (const record of Object.values(manifest)) fileMetrics.set(record.file, { raw: 1, gzip: 1 })
  fileMetrics.set('assets/react-bits.css', { raw: 1, gzip: 1 })
  fileMetrics.set('assets/index.css', { raw: 1, gzip: 1 })

  return {
    developmentJsxFiles: [],
    entryClosure: new Set(['index.html']),
    entryKey: 'index.html',
    fileMetrics,
    initialJsGzip: 1,
    keyByName: new Map(
      Object.entries(manifest)
        .filter(([, record]) => record.name)
        .map(([key, record]) => [record.name, key]),
    ),
    largestJs: { file: 'assets/index.js', raw: 1, gzip: 1 },
    manifest,
    markdownSurfaceGzip: 1,
    records: Object.entries(manifest),
    totalCssGzip: 2,
  }
}

test('bundle budget accepts lazy routes and split Shiki/React Bits assets', () => {
  assert.deepEqual(validateBundle(passingReport()), [])
})

test('bundle budget rejects development JSX runtime output', () => {
  const report = passingReport()
  report.developmentJsxFiles.push('assets/index.js')

  assert.ok(
    validateBundle(report).includes(
      'development JSX runtime emitted in production: assets/index.js',
    ),
  )
})

test('bundle budget rejects size, eager vendor, and CSS ownership regressions', () => {
  const report = passingReport()
  report.largestJs = {
    file: 'assets/regression.js',
    raw: BUNDLE_BUDGETS.largestJsGzip + 1,
    gzip: BUNDLE_BUDGETS.largestJsGzip + 1,
  }
  report.entryClosure.add('_vendor-markdown')
  report.manifest['index.html'].css.push('assets/react-bits.css')

  const failures = validateBundle(report)
  assert.ok(failures.some((failure) => failure.startsWith('largest JS chunk gzip:')))
  assert.ok(failures.includes('route-only vendor is eager: vendor-markdown'))
  assert.ok(failures.includes('React Bits CSS is owned by the application entry'))
})
