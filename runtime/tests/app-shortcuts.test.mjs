import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as shortcuts from '../../shared/shortcuts.mjs'

const compile = async (path) =>
  transformSync(await readFile(path, 'utf8'), { loader: 'tsx', format: 'cjs' }).code
const [componentCode, helpersCode] = await Promise.all([
  compile('src/components/layout/AppShortcuts.tsx'),
  compile('src/lib/shortcuts.ts'),
])

function fixture({
  blocked = false,
  terminal = true,
  bindings = shortcuts.DEFAULT_SHORTCUTS,
  // 外壳只在有处理对象时传入：⌥A 需要有等待中的事项，⌘[ 只在会话页可用。
  attention = true,
  session = true,
  platform = 'Win32',
} = {}) {
  const handlers = new Set()
  const calls = []
  const cleanups = []
  class Element {
    constructor(selector) {
      this.selector = selector
    }
    closest(selectors) {
      return selectors.includes(this.selector) ? this : null
    }
  }
  const window = {
    addEventListener(type, handler) {
      if (type === 'keydown') handlers.add(handler)
    },
    removeEventListener(type, handler) {
      if (type === 'keydown') handlers.delete(handler)
    },
  }
  const document = {
    visibilityState: 'visible',
    dialog: null,
    querySelector() {
      return this.dialog
    },
  }
  const store = { useShortcutStore: (selector) => selector({ bindings }) }
  const environment = { window, document, HTMLElement: Element, navigator: { platform } }
  const evaluate = (source, require) => {
    const module = { exports: {} }
    runInNewContext(source, { ...environment, module, exports: module.exports, require })
    return module.exports
  }
  const helpers = evaluate(helpersCode, (id) => {
    if (id === '@shared/shortcuts.mjs') return shortcuts
    if (id === '@/stores/shortcut-store') return store
    throw new Error(id)
  })
  const { AppShortcuts } = evaluate(componentCode, (id) => {
    if (id === 'react')
      return {
        useEffect(callback) {
          cleanups.push(callback())
        },
      }
    if (id === '@/lib/shortcuts') return helpers
    if (id === '@/stores/shortcut-store') return store
    throw new Error(id)
  })
  AppShortcuts({
    blocked,
    onToggleSidebar: () => calls.push('sidebar'),
    onCommandPalette: () => calls.push('palette'),
    onPrimary: () => calls.push('primary'),
    onSearch: () => calls.push('search'),
    onToggleTerminal: terminal ? () => calls.push('terminal') : undefined,
    onSettings: () => calls.push('settings'),
    onOpenBoard: () => calls.push('board'),
    onOpenComponents: () => calls.push('components'),
    onOpenMore: () => calls.push('more'),
    onJumpToAttention: attention ? () => calls.push('attention') : undefined,
    onBackToBoard: session ? () => calls.push('back') : undefined,
  })
  const press = (code, properties = {}) => {
    const event = {
      code,
      key: code,
      ctrlKey: true,
      shiftKey: false,
      altKey: false,
      metaKey: false,
      defaultPrevented: false,
      preventDefault() {
        this.defaultPrevented = true
      },
      ...properties,
    }
    for (const handler of handlers) handler(event)
    return event
  }
  return {
    calls,
    press,
    document,
    Element,
    handlers,
    unmount: () => cleanups.forEach((cleanup) => cleanup()),
  }
}

test('global bindings dispatch each supported action once and prevent the consumed event', () => {
  const f = fixture()
  for (const [code, action] of [
    ['KeyK', 'palette'],
    ['KeyN', 'primary'],
    ['Slash', 'search'],
    ['KeyB', 'sidebar'],
    ['Backquote', 'terminal'],
    ['Comma', 'settings'],
    ['Digit1', 'board'],
    ['Digit2', 'components'],
    ['Digit3', 'more'],
    ['BracketLeft', 'back'],
  ]) {
    f.calls.length = 0
    assert.equal(f.press(code, { ctrlKey: code !== 'Slash' }).defaultPrevented, true)
    assert.deepEqual(f.calls, [action])
  }
  f.calls.length = 0
  assert.equal(f.press('KeyA', { ctrlKey: false, altKey: true }).defaultPrevented, true)
  assert.deepEqual(f.calls, ['attention'])
  f.unmount()
  assert.equal(f.handlers.size, 0)
})

test('editable controls keep bare typing keys while application chords still work', () => {
  // 应用级组合键在输入框内同样生效（规格 2.8：⌘N 新任务、⌘1/2/3 切标签、⌥A 跳到需要你），
  // 不带修饰键的单键（/ 聚焦搜索）只在焦点不在输入框时生效，不能吞掉正常打字。
  const f = fixture()
  for (const selector of ['input', 'textarea', 'select', '[role="textbox"]', '[contenteditable]']) {
    const target = new f.Element(selector)
    assert.equal(f.press('Slash', { target, ctrlKey: false }).defaultPrevented, false)
    assert.equal(f.press('KeyA', { target, ctrlKey: false }).defaultPrevented, false)
    assert.equal(f.press('Digit1', { target, ctrlKey: false }).defaultPrevented, false)
  }
  assert.deepEqual(f.calls, [])
  const target = new f.Element('textarea')
  for (const [code, properties, action] of [
    ['KeyK', {}, 'palette'],
    ['KeyN', {}, 'primary'],
    ['Digit2', {}, 'components'],
    ['Comma', {}, 'settings'],
    ['KeyA', { ctrlKey: false, altKey: true }, 'attention'],
  ]) {
    f.calls.length = 0
    assert.equal(f.press(code, { target, ...properties }).defaultPrevented, true, code)
    assert.deepEqual(f.calls, [action])
  }
})

test('macOS Option typing inside editable controls is never taken by Alt-only chords', () => {
  // ⌥A 在 macOS 上输入“å”：焦点在输入框内时交还输入框，不能跳走；输入框外仍是“跳到需要你”。
  const mac = fixture({ platform: 'MacIntel' })
  for (const selector of ['input', 'textarea', '[contenteditable]']) {
    const target = new mac.Element(selector)
    const event = mac.press('KeyA', { target, key: 'å', ctrlKey: false, altKey: true })
    assert.equal(event.defaultPrevented, false, selector)
  }
  assert.deepEqual(mac.calls, [])
  assert.equal(mac.press('KeyA', { key: 'å', ctrlKey: false, altKey: true }).defaultPrevented, true)
  assert.deepEqual(mac.calls, ['attention'])
  // ⌘ 组合在 macOS 输入框内照常生效。
  mac.calls.length = 0
  const target = new mac.Element('textarea')
  assert.equal(mac.press('KeyN', { target, ctrlKey: false, metaKey: true }).defaultPrevented, true)
  assert.deepEqual(mac.calls, ['primary'])
})

test('shell actions without a current target leave the key to the page', () => {
  // 没有等待中的事项时 ⌥A 不消费；不在会话页时 ⌘[ 交还浏览器或页面自己的处理。
  const f = fixture({ attention: false, session: false })
  assert.equal(f.press('KeyA', { ctrlKey: false, altKey: true }).defaultPrevented, false)
  assert.equal(f.press('BracketLeft').defaultPrevented, false)
  assert.deepEqual(f.calls, [])
  assert.equal(f.press('Digit1').defaultPrevented, true)
  assert.deepEqual(f.calls, ['board'])
})

test('recorders, terminal input, menus, dialogs, hidden documents and blocked overlays suppress global actions', () => {
  const f = fixture()
  for (const selector of [
    '[data-shortcut-recorder]',
    '.terminal-panel',
    '[role="menu"]',
    '[role="listbox"]',
  ]) {
    assert.equal(f.press('KeyK', { target: new f.Element(selector) }).defaultPrevented, false)
  }
  f.document.dialog = {}
  f.press('KeyK')
  f.document.dialog = null
  f.document.visibilityState = 'hidden'
  f.press('KeyK')
  assert.deepEqual(f.calls, [])
  const overlay = fixture({ blocked: true })
  overlay.press('KeyK')
  assert.deepEqual(overlay.calls, [])
})

test('global shortcuts respect repeat, IME, consumed events and exact modifiers', () => {
  const f = fixture()
  for (const properties of [
    { repeat: true },
    { isComposing: true },
    { keyCode: 229 },
    { defaultPrevented: true },
    { shiftKey: true },
    { altKey: true },
    { metaKey: true },
  ])
    f.press('KeyK', properties)
  assert.deepEqual(f.calls, [])
})

test('disabled and rebound keys never dispatch their old action; unsupported terminal is not consumed', () => {
  const f = fixture({
    terminal: false,
    bindings: { ...shortcuts.DEFAULT_SHORTCUTS, commandPalette: 'F9', primaryAction: null },
  })
  for (const code of ['KeyK', 'KeyN', 'Backquote'])
    assert.equal(f.press(code).defaultPrevented, false)
  f.press('F9', { ctrlKey: false })
  assert.deepEqual(f.calls, ['palette'])
})
