import assert from 'node:assert/strict'
import test from 'node:test'
import {
  BUNDLE_BUDGETS,
  initialApplicationClosure,
  validateBundle,
} from '../../scripts/check-bundle-budget.mjs'

const ROUTE_SOURCES = [
  'src/features/chat/ChatPage.tsx',
  'src/features/chat/ChatHistoryPage.tsx',
  'src/features/assets/AssetsPage.tsx',
  'src/features/channels/ChannelsPage.tsx',
  'src/features/schedules/SchedulesPage.tsx',
  'src/features/config/ConfigPage.tsx',
  'src/features/plugins/PluginsPage.tsx',
  'src/features/memory/MemoryPage.tsx',
  'src/features/mcp/McpPage.tsx',
  'src/features/skills/SkillsPage.tsx',
  'src/features/workflows/WorkflowsPage.tsx',
]

const REACT_BITS_SOURCES = [
  'src/components/react-bits/AnimatedList.tsx',
  'src/components/react-bits/ClickSpark.tsx',
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

test('bootstrap restore counts the unconditional app mount in the initial bundle', () => {
  const manifest = {
    'index.html': { imports: ['_bootstrap'], dynamicImports: ['src/mount-app.tsx'] },
    _bootstrap: { imports: [] },
    'src/mount-app.tsx': { imports: ['_router'] },
    _router: { imports: [] },
  }
  assert.deepEqual(
    initialApplicationClosure(manifest, 'index.html'),
    new Set(['index.html', '_bootstrap', 'src/mount-app.tsx', '_router']),
  )
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

test('the retired split-view vendor is never bundled, lazily or eagerly', () => {
  // Dockview 分屏已退役（规格 v2 第 9.1 节）：原先“必须由懒加载的 ChatDockView 持有”的约束收紧为
  // 任何路由都不得再打出分屏运行时；进入入口依赖图时仍按路由专属依赖报错。
  const report = passingReport()
  assert.deepEqual(validateBundle(report), [])
  report.keyByName.set('vendor-dockview', '_vendor-dockview')
  report.manifest['_vendor-dockview'] = {
    file: 'assets/vendor-dockview.js',
    name: 'vendor-dockview',
    imports: [],
  }
  report.manifest['src/features/chat/ChatPage.tsx'] = {
    ...report.manifest['src/features/chat/ChatPage.tsx'],
    imports: ['_vendor-dockview'],
  }
  assert.ok(validateBundle(report).includes('retired split-view vendor must not be bundled'))
  report.entryClosure.add('_vendor-dockview')
  assert.ok(validateBundle(report).includes('route-only vendor is eager: vendor-dockview'))
})

test('workbench bundle rejects split-view code and decorative welcome styles', () => {
  const report = passingReport()
  report.keyByName.set('vendor-dockview', '_vendor-dockview')
  report.manifest['src/features/chat/WelcomeEffects.tsx'] = {
    file: 'assets/WelcomeEffects.js',
    isDynamicEntry: true,
    css: ['assets/react-bits.css'],
  }

  const failures = validateBundle(report)
  assert.ok(failures.includes('retired split-view vendor must not be bundled'))
  assert.ok(failures.includes('PI welcome must not load decorative React Bits CSS'))
})
