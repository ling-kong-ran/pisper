import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { createBackStack } from '../../src/lib/back-navigation.ts'
test('back closes one top layer then sidebar then task content, without changing history', () => {
  const stack = createBackStack()
  const calls = []
  const removeContent = stack.register(100, () => {
    calls.push('content')
    removeContent()
  })
  const removeSidebar = stack.register(200, () => {
    calls.push('sidebar')
    removeSidebar()
  })
  const removeModal = stack.register(300, () => {
    calls.push('modal')
    removeModal()
  })
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal'])
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal', 'sidebar'])
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal', 'sidebar', 'content'])
  assert.equal(stack.dismiss(), false)
})
test('most recently mounted sibling wins and disposed layers cannot intercept back', () => {
  const stack = createBackStack()
  const calls = []
  stack.register(100, () => calls.push('older'))
  const dispose = stack.register(100, () => calls.push('newer'))
  stack.dismiss()
  assert.deepEqual(calls, ['newer'])
  dispose()
  dispose()
  stack.dismiss()
  assert.deepEqual(calls, ['newer', 'older'])
})

// 原生返回（Android 返回键经 __pisperRequestBack 进入）的分层：先关弹层与返回层，
// 都没有时才交给路由层的上级页面（shellBackPath：更多的子页回更多、一级页回主页、
// 从看板点进的会话走历史回到来处），会话主页是根，路由层不处理时交回平台。
// 键盘 Esc 永远不用于离开页面（规格 2.8）。层级本身见 android-back-navigation / session-home。
const backNavigationCode = transformSync(
  await readFile('src/components/layout/BackNavigation.tsx', 'utf8'),
  { loader: 'tsx', format: 'cjs' },
).code

function backFixture({ onRouteBack, overlay = false } = {}) {
  const stack = createBackStack()
  const listeners = { capture: new Set(), bubble: new Set() }
  class KeyboardEvent {
    constructor(type, init = {}) {
      Object.assign(this, { type, ...init, defaultPrevented: false })
    }
    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true
    }
  }
  const dispatch = (event) => {
    for (const listener of listeners.capture) listener(event)
    for (const listener of listeners.bubble) listener(event)
    return !event.defaultPrevented
  }
  const window = {
    addEventListener(type, listener, capture) {
      if (type === 'keydown') listeners[capture ? 'capture' : 'bubble'].add(listener)
    },
    removeEventListener(type, listener, capture) {
      if (type === 'keydown') listeners[capture ? 'capture' : 'bubble'].delete(listener)
    },
  }
  const overlayElement = {
    dataset: { state: 'open' },
    getAttribute: () => null,
    getClientRects: () => [{}],
  }
  const document = {
    body: { dispatchEvent: dispatch },
    activeElement: null,
    querySelectorAll: () => (fixtureState.overlay ? [overlayElement] : []),
  }
  const fixtureState = { overlay }
  const cleanups = []
  const module = { exports: {} }
  runInNewContext(backNavigationCode, {
    module,
    exports: module.exports,
    window,
    document,
    KeyboardEvent,
    require: (id) => {
      if (id === 'react')
        return {
          useEffect: (effect) => cleanups.push(effect()),
          useRef: (value) => ({ current: value }),
        }
      if (id === '@/lib/back-navigation') return { appBackStack: stack }
      throw new Error(`unexpected dependency ${id}`)
    },
  })
  module.exports.BackNavigation({ onRouteBack })
  return {
    stack,
    state: fixtureState,
    requestBack: () => window.__pisperRequestBack(),
    pressEscape: () => dispatch(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })),
    unmount: () => cleanups.forEach((cleanup) => cleanup?.()),
    window,
  }
}

test('native back closes layers first, then walks up the route hierarchy, then yields to the platform', () => {
  const calls = []
  let routeHandled = true
  const f = backFixture({
    onRouteBack: () => {
      calls.push('route')
      return routeHandled
    },
  })
  const removeSheet = f.stack.register(100, () => {
    calls.push('sheet')
    removeSheet()
  })
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['sheet'])
  // 没有返回层时交给路由层：回到上一级页面（更多的子页回更多、一级页回主页）。
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['sheet', 'route'])
  // 路由层也不处理（已在根：会话主页或从主页落地的会话）时交还平台的默认返回。
  routeHandled = false
  assert.equal(f.requestBack(), false)
  // 打开的弹层由它自己的 Escape 处理，返回不会越过它离开页面。
  f.state.overlay = true
  calls.length = 0
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, [])
  f.unmount()
  assert.equal(f.window.__pisperRequestBack, undefined)
})

test('keyboard Escape only dismisses layers and never leaves the page', () => {
  const calls = []
  const f = backFixture({ onRouteBack: () => calls.push('route') > 0 })
  assert.equal(f.pressEscape(), true)
  assert.deepEqual(calls, [])
  f.stack.register(100, () => calls.push('sheet'))
  assert.equal(f.pressEscape(), false)
  assert.deepEqual(calls, ['sheet'])
  // 桌面不传路由层返回：原生返回在没有返回层时直接交还平台。
  const desktop = backFixture()
  assert.equal(desktop.requestBack(), false)
})
