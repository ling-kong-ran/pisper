import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { matchRoutes } from 'react-router'
import * as routes from '../../src/app/routes.ts'

const { legacyHashPath, pageFromPath, pagePath, workflowPath } = routes
const require = createRequire(import.meta.url)

test('page ids map to stable application paths', () => {
  // 看板是首页：未知页面 id 回到看板；/chat 只保留为兼容入口，由路由层重定向。
  assert.equal(pagePath('board'), '/board')
  assert.equal(pagePath('chat'), '/chat')
  assert.equal(pagePath('chatHistory'), '/chat/history')
  assert.equal(pagePath('workflowCreate'), '/workflows/new')
  assert.equal(pagePath('unknown'), '/board')
})

test('router paths resolve to page ids and tolerate trailing slashes', () => {
  assert.equal(pageFromPath('/config'), 'config')
  assert.equal(pageFromPath('/chat/history/'), 'chatHistory')
  assert.equal(pageFromPath('/workflows/workflow-1'), 'workflowCreate')
  assert.equal(pageFromPath('/unknown'), null)
})

test('workflow editor paths preserve the selected workflow id', () => {
  assert.equal(workflowPath('workflow 1'), '/workflows/workflow%201')
  assert.equal(workflowPath(), '/workflows/new')
})

test('legacy hash urls migrate without changing modern router urls', () => {
  assert.equal(legacyHashPath('#chat'), '/chat')
  assert.equal(legacyHashPath('#workflowCreate'), '/workflows/new')
  assert.equal(legacyHashPath('#/chat'), null)
})

// 真实路由表：把 router.tsx 编译后在隔离上下文里求值，页面装载器与 App 换成占位，
// 用 react-router 自己的匹配器验证每个入口最终落到哪里，而不是只匹配源码文本。
async function routerTable() {
  const code = transformSync(await readFile('src/app/router.tsx', 'utf8'), {
    loader: 'tsx',
    format: 'cjs',
    jsx: 'automatic',
  }).code
  const Navigate = () => null
  const stub = (name) => Object.assign(() => null, { displayName: name })
  const loaders = new Proxy({}, { get: (_target, name) => stub(String(name)) })
  const modules = {
    'react/jsx-runtime': require('react/jsx-runtime'),
    'react-router-dom': { Navigate, createHashRouter: (table) => table },
    '@/App': { default: stub('App') },
    './UnsavedChangesProvider': { UnsavedChangesProvider: stub('UnsavedChangesProvider') },
    '@/components/layout/PageLoader': { PageLoader: stub('PageLoader') },
    './RouteErrorBoundary': { RouteErrorBoundary: stub('RouteErrorBoundary') },
    './routes': routes,
    './route-elements': loaders,
  }
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    require: (id) => {
      if (!(id in modules)) throw new Error(`unexpected router dependency ${id}`)
      return modules[id]
    },
  })
  return { table: module.exports.router, Navigate }
}

test('home, the retired chat page and unknown paths all land on the board', async () => {
  const { table, Navigate } = await routerTable()
  const destination = (path) => {
    const matched = matchRoutes(table, path)
    assert.ok(matched, `${path} must match a route`)
    const leaf = matched.at(-1).route
    if (leaf.element?.type === Navigate) return { redirect: leaf.element.props.to }
    return { lazy: leaf.lazy?.displayName || '' }
  }
  for (const path of ['/', '/chat', '/chat/', '/no-such-page', '/config/models/extra'])
    assert.deepEqual(destination(path), { redirect: '/board' }, path)
  assert.deepEqual(destination('/config'), { redirect: '/config/models' })
  assert.deepEqual(destination('/board'), { lazy: 'boardRoute' })
  assert.deepEqual(destination('/board/abc'), { lazy: 'sessionRoute' })
  assert.deepEqual(destination('/board/new'), { lazy: 'sessionRoute' })
  assert.deepEqual(destination('/more'), { lazy: 'moreRoute' })
  // 组件标签直接渲染组件管理页，不再反向跳到“设置 → 界面 → 独立组件”。
  assert.deepEqual(destination('/components'), { lazy: 'componentsRoute' })
  assert.deepEqual(destination('/tools/components/game-assets'), { lazy: 'customComponentRoute' })
  assert.deepEqual(destination('/chat/history'), { lazy: 'chatHistoryRoute' })
  assert.deepEqual(destination('/config/interface'), { lazy: 'configRoute' })
})

test('the retired widgets settings tab and legacy callers redirect to the components tab', async () => {
  const code = transformSync(await readFile('src/app/ChatAppearancePage.tsx', 'utf8'), {
    loader: 'tsx',
    format: 'cjs',
    jsx: 'automatic',
  }).code
  const Navigate = () => null
  const render = (search) => {
    const module = { exports: {} }
    const modules = {
      react: require('react'),
      'react/jsx-runtime': require('react/jsx-runtime'),
      'react-router-dom': {
        Navigate,
        useSearchParams: () => [new URLSearchParams(search), () => {}],
      },
      '@/app/routes': routes,
      '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
      '@/app/i18n': { ensureCustomUiMessages: async () => {} },
      '@/components/ui/tabs': { Tabs: 'tabs', TabsContent: 'c', TabsList: 'l', TabsTrigger: 't' },
    }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require: (id) => {
        if (!(id in modules)) throw new Error(`unexpected dependency ${id}`)
        return modules[id]
      },
    })
    return module.exports.ChatAppearancePage({ appearance: 'appearance', notify() {} })
  }
  for (const search of ['view=widgets', 'view=layout']) {
    const element = render(search)
    assert.equal(element.type, Navigate, search)
    assert.equal(element.props.to, '/components')
    assert.equal(element.props.replace, true)
  }
  assert.notEqual(render('').type, Navigate)
  // 这个页面就是 /config/interface 实际渲染的界面设置（配置路由把外观区交给它）。
  assert.match(
    await readFile('src/app/route-elements.tsx', 'utf8'),
    /renderInterfaceSettings=\{\(appearance\) => \(\s*<ChatAppearancePage appearance=\{appearance\}/,
  )
  // 设置搜索等旧调用方传 view=widgets 时，应用壳直接去组件页而不是落到界面设置。
  const app = await readFile('src/App.tsx', 'utf8')
  assert.match(
    app,
    /if \(nextSection === 'interface' && view === 'widgets'\) routerNavigate\(PAGE_PATHS\.components\)/,
  )
})
