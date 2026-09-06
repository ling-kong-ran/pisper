import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { DEFAULT_SHORTCUTS, formatShortcut, matchesShortcut } from '../../shared/shortcuts.mjs'

const componentSource = await readFile('src/features/chat/VoiceInputControl.tsx', 'utf8')
const shortcutSource = await readFile('src/features/chat/use-voice-shortcut.ts', 'utf8')
const compiled = new Map(
  [
    ['component', componentSource],
    ['shortcut', shortcutSource],
  ].map(([name, source]) => [
    name,
    transformSync(source, { loader: 'tsx', format: 'cjs', jsx: 'automatic' }).code,
  ]),
)

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

class FakeElement {
  constructor(selector = '') {
    this.selector = selector
  }
  closest(selectors) {
    return this.selector && selectors.includes(this.selector) ? this : null
  }
  getClientRects() {
    return [1]
  }
}

function eventHost() {
  const listeners = new Map()
  return {
    addEventListener(type, callback, capture = false) {
      const list = listeners.get(type) ?? []
      list.push({ callback, capture })
      listeners.set(type, list)
    },
    removeEventListener(type, callback) {
      listeners.set(
        type,
        (listeners.get(type) ?? []).filter((item) => item.callback !== callback),
      )
    },
    emit(type, properties = {}) {
      const event = {
        code: 'Space',
        key: ' ',
        ctrlKey: true,
        shiftKey: true,
        altKey: false,
        metaKey: false,
        repeat: false,
        defaultPrevented: false,
        isComposing: false,
        target: new FakeElement(),
        preventDefault() {
          this.defaultPrevented = true
        },
        ...properties,
      }
      for (const { callback } of [...(listeners.get(type) ?? [])].sort(
        (a, b) => Number(b.capture) - Number(a.capture),
      ))
        callback(event)
      return event
    },
    listeners,
  }
}

// 保留真实组件及 Hook 的执行逻辑，只替换渲染器和浏览器设备，便于确定性重现微任务竞态。
function fixture(t, options = {}) {
  const slots = []
  let cursor = 0
  let effects = []
  let dirty = false
  let mounted = true
  let tree
  let component
  const window = {
    ...eventHost(),
    __PISPER_MOBILE_APP__: Boolean(options.platform || options.android),
    __PISPER_MOBILE_PLATFORM__: options.platform ?? (options.android ? 'android' : undefined),
    setInterval: () => 1,
    clearInterval() {},
    setTimeout: () => 1,
    clearTimeout() {},
  }
  const document = {
    ...eventHost(),
    visibilityState: 'visible',
    get hidden() {
      return this.visibilityState === 'hidden'
    },
    dialogs: [],
    querySelectorAll() {
      return this.dialogs
    },
  }
  const react = {
    useRef(initial) {
      const index = cursor++
      slots[index] ??= { current: initial }
      return slots[index]
    },
    useState(initial) {
      const index = cursor++
      slots[index] ??= { value: initial }
      return [
        slots[index].value,
        (next) => {
          const value = typeof next === 'function' ? next(slots[index].value) : next
          if (!Object.is(value, slots[index].value)) {
            slots[index].value = value
            dirty = true
          }
        },
      ]
    },
    useEffect(callback, deps) {
      const index = cursor++
      const previous = slots[index]
      if (!previous || !deps || deps.some((value, key) => !Object.is(value, previous.deps[key]))) {
        effects.push(() => {
          previous?.cleanup?.()
          slots[index] = { deps, cleanup: callback() }
        })
      }
    },
  }
  const inserted = []
  const recognizers = []
  const captures = []
  const permission = deferred()
  if (!options.permissionPending) permission.resolve()
  const props = {
    sessionId: 'session-one',
    shortcutEnabled: true,
    onInsert: (text) => inserted.push(text),
    ...options.props,
  }
  const store = { bindings: { voiceInput: options.binding ?? 'Mod+Shift+Space' } }
  const microphone = {
    VOICE_MAX_DURATION_SECONDS: 60,
    requestMicrophonePermission: () => permission.promise,
    createSpeechRecognizer() {
      const initialize = deferred()
      const disposal = deferred()
      if (!options.disposalPending) disposal.resolve()
      const recognizer = {
        initialize,
        disposal,
        starts: 0,
        finishes: 0,
        disposes: 0,
        samples: 0,
        start() {
          this.starts += 1
          return initialize.promise
        },
        acceptPcm() {
          this.samples += 1
          return false
        },
        onPartial() {
          return () => {}
        },
        async finish() {
          this.finishes += 1
          await initialize.promise
          return 'dictated text'
        },
        dispose() {
          this.disposes += 1
          return disposal.promise
        },
      }
      recognizers.push(recognizer)
      return recognizer
    },
    startMicrophoneCapture(onPcm, signal) {
      const ready = deferred()
      const capture = {
        ready,
        onPcm,
        signal,
        stops: 0,
        async stop() {
          this.stops += 1
        },
      }
      captures.push(capture)
      if (!options.capturePending) ready.resolve(capture)
      return ready.promise
    },
  }
  const jsx = (type, props) => ({ type, props })
  const modules = {
    react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'fragment' },
    'lucide-react': Object.fromEntries(
      ['ArrowUpLeft', 'LoaderCircle', 'Mic', 'MicOff', 'RotateCcw', 'X'].map((name) => [
        name,
        name,
      ]),
    ),
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/stores/shortcut-store': { useShortcutStore: (selector) => selector(store) },
    '@/lib/shortcuts': {
      formatShortcut: (binding) => formatShortcut(binding, Boolean(options.mac)),
      matchesShortcut: (event, binding) => matchesShortcut(event, binding, Boolean(options.mac)),
    },
    'react-dom': { createPortal: (node) => node },
    './voice-input': microphone,
    './voice-mode-state': {
      createLevelSmoother: () => ({ push: (value) => value, reset() {} }),
      pcmLevel: () => 0,
    },
    // 端点检测替身：永不停顿自动结束，保留人声标记以免走静音丢弃分支。
    './voice-endpoint': {
      createVoiceEndpoint: async () => ({
        acceptPcm: () => false,
        hasSpeech: true,
        reset() {},
        dispose() {},
      }),
    },
    './AnchoredPopupMenu': { AnchoredPopupMenu: 'popup' },
    './SpeechModelsDialog': { SpeechModelsDialog: 'speech-model-dialog' },
    './use-speech-models': {
      useSpeechModels: () => ({
        models: [{ id: 'asr', status: 'installed' }],
        ensureReady: async () => {},
        show() {},
      }),
    },
  }
  const load = (name) => {
    const module = { exports: {} }
    runInNewContext(compiled.get(name), {
      exports: module.exports,
      module,
      require(id) {
        assert.ok(modules[id], `Unexpected module ${id}`)
        return modules[id]
      },
      window,
      document,
      Element: FakeElement,
      DOMException,
      performance,
      AbortController,
      Error,
    })
    return module.exports
  }
  modules['./use-voice-shortcut'] = load('shortcut')
  component = load('component').VoiceInputControl
  const render = () => {
    if (!mounted) return
    do {
      dirty = false
      cursor = 0
      effects = []
      tree = component(props)
      for (const effect of effects) effect()
    } while (dirty)
  }
  const flush = async () => {
    await setImmediate()
    render()
    await setImmediate()
    if (dirty) render()
  }
  const unmount = () => {
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  t.after(unmount)
  render()
  return {
    window,
    document,
    permission,
    recognizers,
    captures,
    inserted,
    flush,
    render,
    unmount,
    update(next) {
      Object.assign(props, next)
      render()
    },
    binding(next) {
      store.bindings.voiceInput = next
      render()
    },
    // 错误胶囊（无 aria-label 的 button）会渲染在麦克风按钮前面，两者都用 aria-label 区分。
    click() {
      tree.props.children
        .find((child) => child && child.type === 'button' && child.props['aria-label'])
        .props.onClick()
    },
    // 面板已移除：取消入口收敛为窗口 blur（与原 popup onClose 同一取消路径）。
    close() {
      window.emit('blur')
    },
    visibility(state) {
      document.visibilityState = state
      document.emit('visibilitychange')
    },
    // 错误态不再渲染面板文本，错误信息挂在麦克风按钮 title 上。
    get errorText() {
      if (slots[0].value !== 'error') return null
      return tree.props.children.find(
        (child) => child && child.type === 'button' && child.props['aria-label'],
      )?.props.title
    },
    get stage() {
      return slots[0].value
    },
    get buttonTitle() {
      return tree.props.children.find(
        (child) => child && child.type === 'button' && child.props['aria-label'],
      ).props.title
    },
  }
}

test('quick release before permission cancels and a late grant never starts capture', async (t) => {
  const f = fixture(t, { permissionPending: true })
  assert.equal(f.window.emit('keydown').defaultPrevented, true)
  f.window.emit('keyup')
  assert.equal(f.stage, 'idle')
  assert.equal(f.recognizers[0].disposes, 1)
  f.permission.resolve()
  await f.flush()
  assert.equal(f.captures.length, 0)
  assert.equal(f.recognizers[0].starts, 0)
  assert.deepEqual(f.inserted, [])
})

test('default F8 is a single-key hold that starts once and transcribes on release', async (t) => {
  assert.equal(DEFAULT_SHORTCUTS.voiceInput, 'F8')
  const f = fixture(t, { binding: DEFAULT_SHORTCUTS.voiceInput })
  const key = { code: 'F8', key: 'F8', ctrlKey: false, shiftKey: false }
  assert.equal(f.window.emit('keydown', key).defaultPrevented, true)
  f.window.emit('keydown', { ...key, repeat: true })
  await f.flush()
  assert.equal(f.recognizers.length, 1)
  assert.equal(f.stage, 'recording')
  f.window.emit('keyup', key)
  assert.equal(f.captures[0].signal.aborted, true)
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.deepEqual(f.inserted, ['dictated text'])
  assert.equal(f.stage, 'idle')
})

test('late capture after release is immediately stopped and cannot restore recording', async (t) => {
  const f = fixture(t, { capturePending: true })
  f.window.emit('keydown')
  await f.flush()
  f.window.emit('keyup')
  assert.equal(f.captures[0].signal.aborted, true)
  f.captures[0].onPcm(new Float32Array([1]))
  assert.equal(f.recognizers[0].samples, 0)
  f.captures[0].ready.resolve(f.captures[0])
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.stage, 'idle')
  assert.deepEqual(f.inserted, [])
})

test('a stale capture cannot stop or reset the next keyboard hold', async (t) => {
  const f = fixture(t, { capturePending: true })
  f.window.emit('keydown')
  await f.flush()
  f.window.emit('keyup')
  f.window.emit('keydown')
  await f.flush()
  f.captures[1].ready.resolve(f.captures[1])
  await f.flush()
  f.captures[0].ready.resolve(f.captures[0])
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.captures[0].signal.aborted, true)
  assert.equal(f.captures[1].stops, 0)
  assert.equal(f.captures[1].signal.aborted, false)
  assert.equal(f.stage, 'recording')
  f.window.emit('keyup')
  f.recognizers[1].initialize.resolve()
  await f.flush()
  assert.deepEqual(f.inserted, ['dictated text'])
})

test('release stops capture synchronously while initializer is pending, then inserts only text', async (t) => {
  const f = fixture(t)
  f.window.emit('keydown')
  await f.flush()
  assert.equal(f.stage, 'recording')
  assert.equal(f.recognizers[0].starts, 1)
  f.window.emit('keyup', { target: new FakeElement('[role="dialog"]'), defaultPrevented: true })
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.captures[0].signal.aborted, true)
  assert.equal(f.stage, 'transcribing')
  await f.flush()
  assert.equal(f.recognizers[0].finishes, 1)
  assert.deepEqual(f.inserted, [])
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.deepEqual(f.inserted, ['dictated text'])
})

test('physical hold and repeated keydown only start once, including before a render', async (t) => {
  const f = fixture(t)
  f.window.emit('keydown')
  f.window.emit('keydown')
  f.window.emit('keydown', { repeat: true })
  assert.equal(f.recognizers.length, 1)
  await f.flush()
  f.window.emit('keyup', { code: 'ShiftLeft', key: 'Shift', shiftKey: false })
  assert.equal(f.captures[0].stops, 1)
  f.recognizers[0].initialize.resolve()
  await f.flush()
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 1)
  f.window.emit('keyup')
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 2)
})

for (const modifier of [
  { code: 'ControlRight', key: 'Control', ctrlKey: false },
  { code: 'ShiftRight', key: 'Shift', shiftKey: false },
]) {
  test(`release of required ${modifier.key} ends recording regardless of key order`, async (t) => {
    const f = fixture(t)
    f.window.emit('keydown')
    await f.flush()
    f.window.emit('keyup', modifier)
    f.window.emit('keyup')
    assert.equal(f.captures[0].stops, 1)
    f.recognizers[0].initialize.resolve()
    await f.flush()
    assert.deepEqual(f.inserted, ['dictated text'])
  })
}

for (const reason of [
  'blur',
  'interruption',
  'hidden',
  'inactive',
  'session',
  'disabled',
  'unmount',
  'binding',
]) {
  test(`${reason} cancels capture and ignores late recognition`, async (t) => {
    const f = fixture(t)
    f.window.emit('keydown')
    await f.flush()
    if (reason === 'blur') f.window.emit('blur')
    if (reason === 'interruption') f.window.emit('pisper:speech-interrupted')
    if (reason === 'hidden') {
      f.document.visibilityState = 'hidden'
      f.document.emit('visibilitychange')
    }
    if (reason === 'inactive') f.update({ shortcutEnabled: false })
    if (reason === 'session') f.update({ sessionId: 'session-two' })
    if (reason === 'disabled') f.update({ disabled: true })
    if (reason === 'unmount') f.unmount()
    if (reason === 'binding') f.binding('Mod+KeyM')
    assert.equal(f.captures[0].stops, 1)
    assert.equal(f.captures[0].signal.aborted, true)
    f.recognizers[0].initialize.resolve()
    f.window.emit('keyup')
    await f.flush()
    assert.deepEqual(f.inserted, [])
  })
}

test('blur while awaiting permission does not resurrect after the permission grant', async (t) => {
  const f = fixture(t, { permissionPending: true })
  f.window.emit('keydown')
  f.window.emit('blur')
  f.permission.resolve()
  await f.flush()
  assert.equal(f.captures.length, 0)
  assert.equal(f.stage, 'idle')
})

test('blur after release cancels pending transcription without inserting late text', async (t) => {
  const f = fixture(t)
  f.window.emit('keydown')
  await f.flush()
  f.window.emit('keyup')
  await f.flush()
  assert.equal(f.recognizers[0].finishes, 1)
  f.window.emit('blur')
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.deepEqual(f.inserted, [])
})

test('initializer failure after release reports an error without resuming capture', async (t) => {
  const f = fixture(t)
  f.window.emit('keydown')
  await f.flush()
  f.window.emit('keyup')
  await f.flush()
  f.recognizers[0].initialize.reject(new Error('recognizer unavailable'))
  await f.flush()
  assert.equal(f.stage, 'error')
  assert.equal(f.captures[0].stops, 1)
  assert.deepEqual(f.inserted, [])
})

for (const platform of ['android', 'ios']) {
  test(
    platform +
      ' mobile touch permission survives hidden and blur, then starts once only after a visible grant',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      f.click()
      await f.flush()
      assert.equal(f.stage, 'requesting')
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.captures.length, 0)

      f.visibility('hidden')
      f.window.emit('blur')
      await f.flush()
      assert.equal(f.stage, 'requesting')
      assert.equal(f.recognizers[0].disposes, 0)
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.captures.length, 0)

      f.visibility('visible')
      f.window.emit('focus')
      await f.flush()
      assert.equal(f.stage, 'requesting')
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.captures.length, 0)

      f.permission.resolve()
      await f.flush()
      f.visibility('visible')
      f.window.emit('focus')
      f.recognizers[0].initialize.resolve()
      await f.flush()
      assert.equal(f.stage, 'recording')
      assert.equal(f.recognizers.length, 1)
      assert.equal(f.recognizers[0].starts, 1)
      assert.equal(f.captures.length, 1)
      assert.equal(f.captures[0].signal.aborted, false)
      assert.equal(f.captures[0].stops, 0)
      assert.deepEqual(f.inserted, [])
    },
  )

  test(
    platform +
      ' mobile touch grant while still hidden closes without capture and visibility cannot revive it',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      f.click()
      f.visibility('hidden')
      await f.flush()
      assert.equal(f.stage, 'requesting')
      f.permission.resolve()
      await f.flush()
      assert.equal(f.stage, 'idle')
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.recognizers[0].disposes, 1)
      assert.equal(f.captures.length, 0)

      f.visibility('visible')
      f.window.emit('focus')
      await f.flush()
      assert.equal(f.stage, 'idle')
      assert.equal(f.recognizers.length, 1)
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.captures.length, 0)
      assert.deepEqual(f.inserted, [])
    },
  )

  test(
    platform +
      ' mobile touch permission denial after visibility returns displays an error without capture',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      f.click()
      f.visibility('hidden')
      f.visibility('visible')
      f.permission.reject(new Error('microphone_permission_denied'))
      await f.flush()
      assert.equal(f.stage, 'error')
      assert.equal(f.errorText, 'chat:voiceInput.permissionDenied')
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.recognizers[0].disposes, 1)
      assert.equal(f.captures.length, 0)
      assert.deepEqual(f.inserted, [])
    },
  )

  test(
    platform + ' explicit close during mobile touch permission invalidates a late grant',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      f.click()
      f.visibility('hidden')
      // 面板已移除：授权等待中的显式取消 = 再次点击按钮（按钮在 requesting 态可点）。
      // 先重渲拿到 requesting 态的 onClick 分支（无面板后按钮闭包随 stage 切换）。
      f.render()
      f.click()
      await f.flush()
      assert.equal(f.stage, 'idle')
      f.visibility('visible')
      f.permission.resolve()
      await f.flush()
      assert.equal(f.stage, 'idle')
      assert.equal(f.recognizers[0].starts, 0)
      assert.equal(f.recognizers[0].disposes, 1)
      assert.equal(f.captures.length, 0)
      assert.deepEqual(f.inserted, [])
    },
  )

  test(
    platform +
      ' mobile touch hidden after native permission cancels pending capture and stops its late result',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true, capturePending: true })
      f.click()
      f.permission.resolve()
      await f.flush()
      assert.equal(f.stage, 'requesting')
      assert.equal(f.recognizers[0].starts, 1)
      assert.equal(f.captures.length, 1)
      assert.equal(f.captures[0].signal.aborted, false)

      f.visibility('hidden')
      assert.equal(f.captures[0].signal.aborted, true)
      assert.equal(f.recognizers[0].disposes, 1)
      f.captures[0].onPcm(new Float32Array([1]))
      assert.equal(f.recognizers[0].samples, 0)
      f.visibility('visible')
      f.captures[0].ready.resolve(f.captures[0])
      f.recognizers[0].initialize.resolve()
      await f.flush()
      assert.equal(f.stage, 'idle')
      assert.equal(f.captures[0].stops, 1)
      assert.equal(f.captures.length, 1)
      assert.equal(f.recognizers[0].starts, 1)
      assert.deepEqual(f.inserted, [])
    },
  )
}

test('desktop touch blur while permission is pending still cancels a late grant', async (t) => {
  const f = fixture(t, { permissionPending: true })
  f.click()
  f.window.emit('blur')
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.equal(f.recognizers[0].disposes, 1)
  f.permission.resolve()
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.equal(f.recognizers[0].starts, 0)
  assert.equal(f.captures.length, 0)
  assert.deepEqual(f.inserted, [])
})

test('click capture also stops on window blur for privacy', async (t) => {
  const f = fixture(t)
  f.click()
  await f.flush()
  f.window.emit('blur')
  assert.equal(f.captures[0].stops, 1)
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.deepEqual(f.inserted, [])
})

test('click remains a toggle and keyboard release cannot stop a click-owned recording', async (t) => {
  const f = fixture(t)
  f.click()
  await f.flush()
  f.window.emit('keydown')
  f.window.emit('keyup')
  assert.equal(f.captures[0].stops, 0)
  assert.equal(f.recognizers.length, 1)
  f.click()
  assert.equal(f.captures[0].stops, 1)
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.deepEqual(f.inserted, ['dictated text'])
})

test('blur close cancels a hold, and its later keyup cannot cancel a new click recording', async (t) => {
  const f = fixture(t)
  f.window.emit('keydown')
  await f.flush()
  f.close()
  f.render()
  f.click()
  await f.flush()
  f.window.emit('keyup')
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.captures[1].stops, 0)
  assert.equal(f.stage, 'recording')
})

test('late cleanup of a completed generation cannot reset a newer recording', async (t) => {
  const f = fixture(t, { disposalPending: true })
  f.window.emit('keydown')
  await f.flush()
  f.window.emit('keyup')
  f.recognizers[0].initialize.resolve()
  await f.flush()
  assert.deepEqual(f.inserted, ['dictated text'])
  f.close()
  f.render()
  f.click()
  await f.flush()
  f.recognizers[0].disposal.resolve()
  await f.flush()
  assert.equal(f.stage, 'recording')
  assert.equal(f.captures[1].stops, 0)
})

for (const blocked of [
  { defaultPrevented: true },
  { isComposing: true },
  { keyCode: 229 },
  { repeat: true },
  { target: new FakeElement('[data-shortcut-recorder]') },
  { target: new FakeElement('[role="dialog"]') },
  { target: new FakeElement('[role="alertdialog"]') },
  { target: new FakeElement('.xterm') },
  { target: new FakeElement('.terminal-panel') },
  { target: new FakeElement('[role="menu"]') },
  { target: new FakeElement('[role="listbox"]') },
  { key: 'Dead' },
  { key: 'Process' },
  { getModifierState: (modifier) => modifier === 'AltGraph' },
  { ctrlKey: false },
]) {
  test(`shortcut ignores blocked keydown ${JSON.stringify(blocked)}`, (t) => {
    const f = fixture(t)
    f.window.emit('keydown', blocked)
    assert.equal(f.recognizers.length, 0)
  })
}

test('Escape cancels even before the recording popup has mounted', async (t) => {
  const f = fixture(t, { permissionPending: true })
  f.window.emit('keydown')
  f.window.emit('keydown', { key: 'Escape', code: 'Escape' })
  f.permission.resolve()
  await f.flush()
  assert.equal(f.stage, 'idle')
  assert.equal(f.captures.length, 0)
})

for (const shortcut of [
  {
    binding: 'Mod+Shift+Space',
    mac: true,
    press: { metaKey: true, ctrlKey: false },
    release: { code: 'MetaLeft', key: 'Meta', metaKey: false },
  },
  {
    binding: 'Alt+KeyM',
    press: { code: 'KeyM', key: 'm', altKey: true, ctrlKey: false, shiftKey: false },
    release: { code: 'AltRight', key: 'Alt', altKey: false },
  },
  {
    binding: 'Ctrl+KeyM',
    mac: true,
    press: { code: 'KeyM', key: 'm', ctrlKey: true, shiftKey: false },
    release: { code: 'ControlLeft', key: 'Control', ctrlKey: false },
  },
]) {
  test(`${shortcut.binding} uses shared matching with mac=${Boolean(shortcut.mac)}`, async (t) => {
    const f = fixture(t, shortcut)
    assert.equal(f.window.emit('keydown', shortcut.press).defaultPrevented, true)
    await f.flush()
    f.window.emit('keyup', { ...shortcut.press, ...shortcut.release })
    assert.equal(f.captures[0].stops, 1)
    f.recognizers[0].initialize.resolve()
    await f.flush()
    assert.deepEqual(f.inserted, ['dictated text'])
  })
}

test('visible modal blocks shortcuts even when focus has not moved into it', (t) => {
  const f = fixture(t)
  f.document.dialogs.push(new FakeElement())
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 0)
})

test('shortcut is opt-in, can be unset, and advertises the configured chord', (t) => {
  const f = fixture(t, { props: { shortcutEnabled: undefined } })
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 0)
  assert.match(f.buttonTitle, /Ctrl \+ Shift \+ Space/)
  f.update({ shortcutEnabled: true })
  f.binding(null)
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 0)
  assert.equal(f.buttonTitle, 'chat:voiceInput.open')
  f.binding('Mod+KeyM')
  f.window.emit('keydown')
  assert.equal(f.recognizers.length, 0)
  f.window.emit('keydown', { code: 'KeyM', key: 'm', shiftKey: false })
  assert.equal(f.recognizers.length, 1)
})
