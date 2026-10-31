import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { uiTextSizes } from '../../src/app/ui-preferences.ts'
import * as utils from '../../src/lib/utils.ts'

const require = createRequire(import.meta.url)

test('the v2 sidebar has a fixed width instead of a draggable, persisted one', async () => {
  // 规格 v2 第 2.1 节：侧栏宽 268、窄轨 60，不可拖动；旧侧栏的拖拽宽度（sidebar-width）随旧外壳删除，
  // 收起偏好只有 collapsed / doneCollapsed 两个布尔值（sidebar-model / sidebar-store 的测试另见）。
  const [shell, store] = await Promise.all([
    readFile('src/app/ShellSidebar.tsx', 'utf8'),
    readFile('src/stores/sidebar-store.ts', 'utf8'),
  ])
  assert.match(shell, /collapsed \? 'w-\[60px\]' : 'w-\[268px\]'/)
  assert.doesNotMatch(shell, /pointermove|cursor-col-resize|role="separator"|sidebarWidth/)
  // 偏好的序列化只写两个布尔值（不落盘任何宽度）。
  assert.match(
    store,
    /return JSON\.stringify\(\{\s*collapsed: preference\.collapsed === true,\s*doneCollapsed: preference\.doneCollapsed === true,\s*\}\)/,
  )
  await assert.rejects(readFile('src/lib/sidebar-width.ts', 'utf8'), { code: 'ENOENT' })
})

test('UI font preferences round their scale without changing content and code font roles', () => {
  // 规格 6.4 字号阶梯：元信息 12 / 密集控件 13 / 卡片标题 14 / 面板 h1 16 / 桌面页 h1 20 /
  // 手机 h1 24 / 数字主值 28；大号收在 28 以内，小号与大号按 14px 基准等比取整。
  assert.deepEqual(uiTextSizes('default'), [12, 13, 14, 16, 20, 24, 28])
  assert.deepEqual(uiTextSizes('small'), [12, 13, 14, 15, 19, 23, 27])
  assert.deepEqual(uiTextSizes('large'), [12, 13, 15, 17, 21, 25, 29])
  assert.deepEqual(uiTextSizes('retired-preference'), uiTextSizes('default'))
})

// —— 手机外壳顶栏（规格 v2 第 7.1 节）：底部标签栏退役，左 ☰ 带琥珀需要你计数，右 ✎ ——
// 真实组件在隔离上下文里渲染：需要你计数换成固定数字，其余依赖用真实模块。
const compile = async (file) =>
  transformSync(await readFile(file, 'utf8'), { loader: 'tsx', format: 'cjs', jsx: 'automatic' })
    .code
const menuButtonCode = await compile('src/app/PhoneMenuButton.tsx')
const topBarCode = await compile('src/app/PhoneTopBar.tsx')
const i18n = {
  useI18n: () => ({ t: (key, values) => (values ? `${key}(${JSON.stringify(values)})` : key) }),
}

function load(code, modules) {
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    require: (id) => {
      if (!(id in modules)) throw new Error(`unexpected phone shell dependency ${id}`)
      return modules[id]
    },
  })
  return module.exports
}

function menuButtonModule({ attention = 0, drawerOpen = false } = {}) {
  return load(menuButtonCode, {
    react: require('react'),
    'react/jsx-runtime': require('react/jsx-runtime'),
    'lucide-react': require('lucide-react'),
    '@/app/use-i18n': i18n,
    '@/lib/utils': utils,
    // 计数与侧栏同一份分组模型（看板 store）。
    '@/features/board/public': {
      useSidebarModel: () => ({ model: { counts: { attention } } }),
    },
    '@/stores/sidebar-store': {
      useSidebarStore: (select) => select({ drawerOpen, setDrawerOpen() {} }),
    },
  })
}

const renderMenuButton = (options) =>
  renderToStaticMarkup(h(menuButtonModule(options).PhoneMenuButton))

function renderTopBar(props, options) {
  const { PhoneTopBar } = load(topBarCode, {
    react: require('react'),
    'react/jsx-runtime': require('react/jsx-runtime'),
    'lucide-react': require('lucide-react'),
    '@/app/use-i18n': i18n,
    './PhoneMenuButton': menuButtonModule(options),
  })
  return renderToStaticMarkup(h(PhoneTopBar, { title: '全部代理', onNewTask() {}, ...props }))
}

test('the phone menu button is a 44px drawer trigger with the amber needs-you count beside ☰', () => {
  const calm = renderMenuButton()
  assert.match(calm, /<button type="button" data-phone-menu="true" aria-haspopup="dialog"/)
  assert.match(calm, /aria-expanded="false"/)
  assert.match(calm, /aria-label="navigation:phone\.menu"/)
  assert.match(calm, /class="[^"]*\bh-11 min-w-11\b/)
  assert.doesNotMatch(calm, /data-phone-attention|st-attn/)
  const busy = renderMenuButton({ attention: 3, drawerOpen: true })
  assert.match(busy, /aria-expanded="true"/)
  // 计数本身对读屏隐藏，数量并进按钮的无障碍名称；琥珀只在这里表示需要你。
  assert.match(busy, /aria-label="navigation:phone\.menuAttention\(\{&quot;count&quot;:3\}\)"/)
  assert.match(busy, /aria-hidden="true" data-phone-attention="3" class="[^"]*\bbg-warning-soft\b/)
  assert.match(renderMenuButton({ attention: 120 }), />99\+<\/span>/)
})

test('the phone top bar is ☰ · centred title · ✎, with search only where the page asks for it', () => {
  const html = renderTopBar({}, { attention: 2 })
  assert.match(
    html,
    /<header data-phone-topbar="true" data-focus-region="header" class="[^"]*\bh-14\b/,
  )
  assert.match(html, /grid-cols-\[minmax\(0,1fr\)_auto_minmax\(0,1fr\)\]/)
  // 页名只是看得见的文字（读屏用每页自己的 h1），避免一页两个 h1。
  assert.match(
    html,
    /<p data-phone-title="true" aria-hidden="true" class="[^"]*\btext-center\b[^"]*">全部代理<\/p>/,
  )
  assert.doesNotMatch(html, /<h1\b/)
  assert.ok(html.indexOf('data-phone-menu') < html.indexOf('data-phone-title'))
  assert.match(
    html,
    /data-phone-new-task="true" aria-label="navigation:shell\.newTask" class="[^"]*\bsize-11\b/,
  )
  assert.doesNotMatch(html, /data-phone-search/)
  const board = renderTopBar({ onSearch() {} })
  assert.ok(board.indexOf('data-phone-search') < board.indexOf('data-phone-new-task'))
  // 只有入口与计数，没有标签、没有批准。
  assert.doesNotMatch(board, /data-shell-tab|mobile-tab-bar/)
})
