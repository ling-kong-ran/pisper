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
  const nativeBack = new WeakSet()
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
      if (id === '@/lib/back-navigation')
        return { appBackStack: stack, markNativeBack: (event) => nativeBack.add(event) }
      throw new Error(`unexpected dependency ${id}`)
    },
  })
  module.exports.BackNavigation({ onRouteBack })
  return {
    stack,
    state: fixtureState,
    requestBack: () => window.__pisperRequestBack(),
    pressEscape: (init = {}) =>
      dispatch(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true, ...init })),
    unmount: () => cleanups.forEach((cleanup) => cleanup?.()),
    window,
    document,
    dispatch,
    nativeBack,
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

// 设置整页的“← 返回 Esc”（规格 2.3）是“Esc 不离开页面”的唯一例外：useEscapeLeave 只在按键没被别人
// 处理、没有弹层、焦点不在输入框、没有修饰键与输入法组合时离开；有返回层时 BackNavigation 先接住。
const escapeLeaveCode = transformSync(await readFile('src/hooks/useEscapeLeave.ts', 'utf8'), {
  loader: 'ts',
  format: 'cjs',
}).code

function mountEscapeLeave(f, onLeave) {
  const cleanups = []
  const module = { exports: {} }
  runInNewContext(escapeLeaveCode, {
    module,
    exports: module.exports,
    window: f.window,
    WeakSet,
    require: (id) => {
      if (id === 'react')
        return {
          useEffect: (effect) => cleanups.push(effect()),
          useRef: (value) => ({ current: value }),
        }
      if (id === '@/lib/back-navigation')
        return {
          hasOpenOverlay: () => f.state.overlay,
          isNativeBack: (event) => f.nativeBack.has(event),
        }
      if (id === '@/lib/shortcuts')
        return { isEditableTarget: (target) => Boolean(target?.editable) }
      throw new Error(`unexpected dependency ${id}`)
    },
  })
  module.exports.useEscapeLeave(onLeave)
  return () => cleanups.forEach((cleanup) => cleanup?.())
}

test('the settings page Escape leaves only when nothing else claims the key', () => {
  const calls = []
  const f = backFixture()
  const unmount = mountEscapeLeave(f, () => calls.push('leave'))
  // 焦点在输入框（设置搜索、API Key 输入）、带修饰键、输入法组合中：不离开。
  assert.equal(f.pressEscape({ target: { editable: true } }), true)
  assert.equal(f.pressEscape({ shiftKey: true }), true)
  assert.equal(f.pressEscape({ isComposing: true }), true)
  assert.deepEqual(calls, [])
  // 有打开着的弹层：Esc 交给弹层。
  f.state.overlay = true
  assert.equal(f.pressEscape(), true)
  assert.deepEqual(calls, [])
  f.state.overlay = false
  // 有返回层（例如列表详情的窄屏详情）：BackNavigation 先关它，这一下不离开设置。
  const remove = f.stack.register(100, () => {
    calls.push('layer')
    remove()
  })
  assert.equal(f.pressEscape(), false)
  assert.deepEqual(calls, ['layer'])
  // 什么都没有接住：离开设置，并标记已处理。
  assert.equal(f.pressEscape(), false)
  assert.deepEqual(calls, ['layer', 'leave'])
  assert.equal(f.pressEscape({ key: 'Enter' }), true)
  unmount()
  assert.equal(f.pressEscape(), true)
  assert.deepEqual(calls, ['layer', 'leave'])
})

// 平板 App 的设置外壳：原生返回合成的 Escape 先被 useEscapeLeave 接住，按原路退回（leaveSettings），
// 不落到 shellBackPath 的“一级页回主页”；焦点在输入框里（设置搜索）也一样，键盘 Esc 在输入框里仍属于输入框。
test('native back in the settings shell leaves settings the way it came, even from a text field', () => {
  const calls = []
  const f = backFixture({ onRouteBack: () => calls.push('route') > 0 })
  const unmount = mountEscapeLeave(f, () => calls.push('leave'))
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['leave'])
  const field = { editable: true }
  field.dispatchEvent = (event) => {
    event.target = field
    return f.dispatch(event)
  }
  f.document.activeElement = field
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['leave', 'leave'])
  // 键盘 Esc 在输入框里不离开设置。
  assert.equal(f.pressEscape({ target: field }), true)
  assert.deepEqual(calls, ['leave', 'leave'])
  // 有返回层时先关它，这一下不离开。
  const remove = f.stack.register(100, () => {
    calls.push('layer')
    remove()
  })
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['leave', 'leave', 'layer'])
  unmount()
  // 设置外壳卸载后（手机设置等）原生返回交给路由层。
  f.document.activeElement = null
  assert.equal(f.requestBack(), true)
  assert.deepEqual(calls, ['leave', 'leave', 'layer', 'route'])
})
