import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { uiTextSizes } from '../../src/app/ui-preferences.ts'
import { PHONE_SHELL_TABS } from '../../src/app/shell-tabs.ts'
import * as laneIcons from '../../src/components/app/LaneBarsIcon.tsx'
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

// —— 第 4 期手机底部标签栏（规格 7.1，主页改为会话）——
// 真实组件在隔离上下文里渲染：需要你计数换成固定数字，其余依赖用真实模块。
const tabBarCode = transformSync(await readFile('src/app/ShellTabBar.tsx', 'utf8'), {
  loader: 'tsx',
  format: 'cjs',
  jsx: 'automatic',
}).code

function renderTabBar({ attention = 0, ...props } = {}) {
  const module = { exports: {} }
  const modules = {
    react: require('react'),
    'react/jsx-runtime': require('react/jsx-runtime'),
    'lucide-react': require('lucide-react'),
    '@/app/use-i18n': {
      useI18n: () => ({
        t: (key, values) => (values ? `${key}(${JSON.stringify(values)})` : key),
      }),
    },
    '@/components/app/LaneBarsIcon': laneIcons,
    '@/lib/utils': utils,
    '@/features/board/public': {
      // 计数与侧栏同一份分组模型（看板 store）。
      useSidebarModel: () => ({
        model: {
          counts: { attention },
          longestWaitingId: attention ? 'waiting-1' : null,
        },
      }),
    },
  }
  runInNewContext(tabBarCode, {
    module,
    exports: module.exports,
    require: (id) => {
      if (!(id in modules)) throw new Error(`unexpected tab bar dependency ${id}`)
      return modules[id]
    },
  })
  return renderToStaticMarkup(
    h(module.exports.ShellTabBar, {
      activeTab: null,
      onSelectTab() {},
      onNewTask() {},
      ...props,
    }),
  )
}

const tabsOf = (html) => [...html.matchAll(/data-shell-tab="(\w+)"/g)].map((match) => match[1])
const badgeOf = (html, tab) => {
  const start = html.indexOf(`data-shell-tab="${tab}"`)
  const button = html.slice(html.lastIndexOf('<button', start), html.indexOf('</button>', start))
  return button.match(/bg-st-attn-solid[^>]*>([^<]+)</)?.[1] ?? null
}

test('the phone tab bar is session · board · new task · more · settings, with the components page inside More', () => {
  const html = renderTabBar({ activeTab: 'board' })
  assert.deepEqual(tabsOf(html), [...PHONE_SHELL_TABS])
  assert.deepEqual([...PHONE_SHELL_TABS], ['session', 'board', 'more', 'settings'])
  assert.doesNotMatch(html, /data-shell-tab="components"/)
  // 居中的“新任务”在看板与更多之间：52×40、主操作色 --star，不是标签（没有 aria-current）。
  const newTask = html.indexOf('aria-label="navigation:shell.newTask"')
  assert.ok(newTask > html.indexOf('data-shell-tab="board"'))
  assert.ok(newTask < html.indexOf('data-shell-tab="more"'))
  assert.match(
    html,
    /<button type="button" aria-label="navigation:shell\.newTask" class="[^"]*\bh-10 w-\[52px\][^"]*\bbg-star\b/,
  )
  // 标签 56px 高（触控目标 ≥44），整栏让出底部安全区，软键盘打开时整栏收起。
  assert.equal(html.match(/data-shell-tab="\w+"[^>]*class="[^"]*\bh-14\b/g)?.length, 4)
  assert.match(html, /<nav aria-label="navigation:shell\.primaryNavigation"/)
  assert.match(html, /pb-\[var\(--pisper-safe-area-bottom\)\]/)
  assert.match(html, /\[\[data-mobile-keyboard=&#x27;open&#x27;\]_&amp;\]:hidden/)
  // 当前标签只有一个，用 aria-current 表达；没有归属标签的页面不点亮任何标签。
  assert.deepEqual(
    [
      ...html.matchAll(
        /aria-current="page" data-shell-tab="(\w+)"|data-shell-tab="(\w+)" aria-current="page"/g,
      ),
    ].map((match) => match[1] || match[2]),
    ['board'],
  )
  assert.doesNotMatch(renderTabBar({ activeTab: null }), /aria-current/)
})

test('the board tab carries the amber needs-you badge on every page, and only when something needs you', () => {
  for (const activeTab of ['board', 'more', 'settings', 'session', null]) {
    const html = renderTabBar({ attention: 3, activeTab })
    assert.equal(badgeOf(html, 'board'), '3', String(activeTab))
    for (const other of ['session', 'more', 'settings'])
      assert.equal(badgeOf(html, other), null, `${other} on ${activeTab}`)
    // 徽标本身对读屏隐藏，数量并进看板标签的无障碍名称。
    assert.match(
      html,
      /aria-label="navigation:shell\.board, navigation:shell\.tabBarAttention\(\{&quot;count&quot;:3\}\)"/,
    )
    assert.match(
      html,
      /aria-hidden="true" class="[^"]*\bbg-st-attn-solid\b[^"]*\bring-st-attn-ring\b/,
    )
  }
  assert.equal(badgeOf(renderTabBar({ attention: 120 }), 'board'), '99+')
  const calm = renderTabBar({ attention: 0, activeTab: 'more' })
  assert.equal(badgeOf(calm, 'board'), null)
  assert.doesNotMatch(calm, /st-attn|tabBarAttention/)
})

test('session pages hide the tab bar entirely', () => {
  // 会话页与主页（规格 7.3）：不渲染任何标签与新任务按钮；组件仍挂着更新 ⌥A 的目标（见 phone-session-page.test）。
  assert.equal(renderTabBar({ attention: 2, hidden: true, activeTab: 'session' }), '')
})
