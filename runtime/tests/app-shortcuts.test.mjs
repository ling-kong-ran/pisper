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
  const environment = { window, document, HTMLElement: Element, navigator: { platform: 'Win32' } }
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
  ]) {
    f.calls.length = 0
    assert.equal(f.press(code, { ctrlKey: code !== 'Slash' }).defaultPrevented, true)
    assert.deepEqual(f.calls, [action])
  }
  f.unmount()
  assert.equal(f.handlers.size, 0)
})

test('editable controls retain typing and primary-action keys without blocking the palette', () => {
  const f = fixture()
  for (const selector of ['input', 'textarea', 'select', '[role="textbox"]', '[contenteditable]']) {
    const target = new f.Element(selector)
    assert.equal(f.press('KeyN', { target }).defaultPrevented, false)
    assert.equal(f.press('Slash', { target, ctrlKey: false }).defaultPrevented, false)
  }
  assert.deepEqual(f.calls, [])
  f.press('KeyK', { target: new f.Element('textarea') })
  assert.deepEqual(f.calls, ['palette'])
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
