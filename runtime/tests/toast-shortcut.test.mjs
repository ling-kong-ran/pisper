import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { parse } from '@babel/parser'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ToastProvider, ToastViewport } from '../../src/components/ui/toast.tsx'

const require = createRequire(import.meta.url)
const radixRequire = createRequire(require.resolve('radix-ui'))
const [appSource, radixSource, ...locales] = await Promise.all([
  readFile(new URL('../../src/App.tsx', import.meta.url), 'utf8'),
  readFile(radixRequire.resolve('@radix-ui/react-toast'), 'utf8'),
  ...['en-US', 'zh-CN'].map(async (language) => ({
    language,
    messages: JSON.parse(
      await readFile(new URL(`../../src/locales/${language}/common.json`, import.meta.url), 'utf8'),
    ),
  })),
])

function findNodes(node, predicate) {
  if (!node || typeof node !== 'object') return []
  const matches = predicate(node) ? [node] : []
  for (const value of Object.values(node)) {
    if (Array.isArray(value)) {
      for (const child of value) matches.push(...findNodes(child, predicate))
    } else if (value && typeof value === 'object') matches.push(...findNodes(value, predicate))
  }
  return matches
}

const appAst = parse(appSource, { sourceType: 'module', plugins: ['typescript', 'jsx'] })
const radixAst = parse(radixSource, { sourceType: 'unambiguous' })
const viewports = findNodes(
  appAst,
  (node) => node.type === 'JSXOpeningElement' && node.name.name === 'ToastViewport',
)
assert.equal(viewports.length, 1, 'App must configure its actual toast viewport')

function appViewportProps(messages) {
  const props = {}
  for (const attribute of viewports[0].attributes) {
    assert.equal(attribute.type, 'JSXAttribute', 'spread props could override the hotkey policy')
    const value = attribute.value
    assert.equal(value?.type, 'JSXExpressionContainer')
    props[attribute.name.name] = runInNewContext(
      `(${appSource.slice(value.expression.start, value.expression.end)})`,
      {
        t(key) {
          assert.equal(key, 'common:ui.notifications')
          return messages['ui.notifications']
        },
      },
    )
  }
  return props
}

const viewportDeclarations = findNodes(
  radixAst,
  (node) => node.type === 'VariableDeclarator' && node.id.name === 'ToastViewport',
)
assert.equal(viewportDeclarations.length, 1)
const keydownEffects = findNodes(
  viewportDeclarations[0],
  (node) =>
    node.type === 'CallExpression' &&
    node.callee.type === 'MemberExpression' &&
    node.callee.property.name === 'useEffect' &&
    findNodes(
      node.arguments[0],
      (child) =>
        child.type === 'CallExpression' &&
        child.callee.type === 'MemberExpression' &&
        child.callee.object.name === 'document' &&
        child.callee.property.name === 'addEventListener' &&
        child.arguments[0]?.value === 'keydown',
    ).length > 0,
)
assert.equal(keydownEffects.length, 1, 'execute the installed viewport document keydown effect')
const keydownEffect = keydownEffects[0].arguments[0]
const defaultHotkeys = findNodes(
  radixAst,
  (node) => node.type === 'VariableDeclarator' && node.id.name === 'VIEWPORT_DEFAULT_HOTKEY',
)
assert.equal(defaultHotkeys.length, 1)
const defaultHotkey = runInNewContext(
  radixSource.slice(defaultHotkeys[0].init.start, defaultHotkeys[0].init.end),
)

function installedHotkeyFixture(t, hotkey) {
  const handlers = new Set()
  let focusCount = 0
  // 执行依赖原始 effect，避免测试复制匹配算法后与实际行为一起失真；只替换 DOM 和调试命名辅助函数。
  const effect = runInNewContext(`(${radixSource.slice(keydownEffect.start, keydownEffect.end)})`, {
    hotkey,
    ref: { current: { focus: () => focusCount++ } },
    __name: (value) => value,
    document: {
      addEventListener(type, handler) {
        assert.equal(type, 'keydown')
        handlers.add(handler)
      },
      removeEventListener(type, handler) {
        assert.equal(type, 'keydown')
        assert.equal(handlers.delete(handler), true)
      },
    },
  })
  const cleanup = effect()
  assert.equal(handlers.size, 1)
  t.after(() => {
    cleanup()
    assert.equal(handlers.size, 0)
  })
  return {
    press(code, properties = {}) {
      const event = {
        code,
        key: code,
        ctrlKey: false,
        altKey: false,
        metaKey: false,
        shiftKey: false,
        defaultPrevented: false,
        preventDefault() {
          this.defaultPrevented = true
        },
        ...properties,
      }
      for (const handler of handlers) handler(event)
      return event
    },
    get focusCount() {
      return focusCount
    },
  }
}

for (const { language, messages } of locales) {
  test(`App explicitly disables toast hotkeys and renders a localized label without F8 (${language})`, () => {
    const props = appViewportProps(messages)
    assert.ok(Array.isArray(props.hotkey))
    assert.equal(props.hotkey.length, 0)
    assert.equal(typeof props.label, 'string')
    assert.ok(props.label.trim())
    assert.equal(props.label, messages['ui.notifications'])
    const html = renderToStaticMarkup(
      React.createElement(ToastProvider, null, React.createElement(ToastViewport, props)),
    )
    assert.ok(html.includes(`aria-label="${props.label}"`))
    assert.match(html, /role="region"/)
    assert.doesNotMatch(html, /F8|\{hotkey\}|\(\)/)
  })
}

test('installed Radix default responds to F8, proving the actual listener can focus', (t) => {
  const f = installedHotkeyFixture(t, defaultHotkey)
  f.press('KeyA')
  assert.equal(f.focusCount, 0)
  assert.equal(f.press('F8').defaultPrevented, false)
  assert.equal(f.focusCount, 1)
  f.press('F8', { defaultPrevented: true })
  assert.equal(f.focusCount, 2)
})

test('installed Radix empty hotkey never focuses for F8, ordinary keys or modifiers', (t) => {
  const f = installedHotkeyFixture(t, appViewportProps(locales[0].messages).hotkey)
  const keys = [
    'F8',
    'KeyA',
    'Enter',
    'Space',
    'Tab',
    'ControlLeft',
    'ShiftLeft',
    'AltLeft',
    'MetaLeft',
  ]
  for (const code of keys) {
    for (const properties of [
      {},
      { ctrlKey: true },
      { shiftKey: true },
      { altKey: true },
      { metaKey: true },
      { ctrlKey: true, shiftKey: true, altKey: true, metaKey: true },
      { repeat: true },
      { isComposing: true },
      { defaultPrevented: true },
    ]) {
      const event = f.press(code, properties)
      assert.equal(f.focusCount, 0, `${code}: ${JSON.stringify(properties)}`)
      assert.equal(event.defaultPrevented, properties.defaultPrevented ?? false)
    }
  }
})
