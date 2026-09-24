import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { parse } from 'parse5'
import { BUILTIN_CUSTOM_UI_COMPONENTS } from '../services/custom-ui-builtins.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { handleCustomUiResource } from '../http/routes/custom-ui.mjs'

function inspectHtml(html) {
  const nodes = []
  const visit = (node) => {
    nodes.push(node)
    for (const child of node.childNodes ?? []) visit(child)
  }
  visit(parse(html))
  return nodes
}

function captureResponse(action) {
  let status = 0
  let headers = {}
  let body
  const res = {
    writeHead(code, values) {
      status = code
      headers = values
    },
    end(value) {
      body = value
    },
  }
  return Promise.resolve(
    action(res, (code, value) => {
      status = code
      body = value
    }),
  ).then(() => ({ status, headers, body }))
}

test('built-in island is available offline without populating the user directory and reserves its ID', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-builtins-'))
  const service = new CustomUiService({ dataDir: directory })
  t.after(async () => {
    service.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const result = await service.listComponents()
  assert.equal(result.components.length, 1)
  assert.equal(result.components[0].id, 'pisper-island')
  assert.equal(result.components[0].builtIn, true)
  assert.equal(result.components[0].directory, '')
  assert.deepEqual(result.components[0].permissions, ['notify'])
  await assert.rejects(stat(service.root), { code: 'ENOENT' })
  const userDirectory = join(service.root, 'pisper-island')
  await mkdir(userDirectory, { recursive: true })
  await writeFile(join(userDirectory, 'manifest.json'), JSON.stringify({ name: 'Pretend builtin' }))
  await writeFile(join(userDirectory, 'index.html'), 'user-owned content')
  await writeFile(join(userDirectory, 'private.txt'), 'not an island asset')
  assert.equal((await service.listComponents()).components.length, 1)
  assert.equal((await service.readManifest('pisper-island')).name, 'Pisper Island')
  assert.notEqual(
    (await service.resolveAssetPath('pisper-island', 'index.html')).content,
    'user-owned content',
  )
  assert.equal(await service.resolveAssetPath('pisper-island', 'private.txt'), null)
  assert.equal(await readFile(join(userDirectory, 'index.html'), 'utf8'), 'user-owned content')
})

test('built-in assets reuse scoped view credentials and retain CSP and path restrictions', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-builtin-views-'))
  let now = 0
  const service = new CustomUiService({ dataDir: directory, now: () => now })
  t.after(async () => {
    service.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const view = await service.createView('pisper-island', 'local', 'http://localhost')
  const request = () =>
    captureResponse((res, json) =>
      handleCustomUiResource({ method: 'GET' }, res, new URL(view.entryUrl, 'http://localhost'), {
        customUi: service,
        json,
      }),
    )
  const response = await request()
  assert.equal(response.status, 200)
  assert.match(response.headers['Content-Type'], /text\/html/)
  assert.equal(response.headers['Content-Length'], Buffer.byteLength(response.body))
  assert.match(response.headers['Content-Security-Policy'], /sandbox allow-scripts/)
  assert.doesNotMatch(response.headers['Content-Security-Policy'], /allow-same-origin/)
  assert.ok(response.body.includes(`http://localhost/api/custom-ui/render/${view.id}/bridge.js`))
  for (const path of [
    'manifest.json',
    '../index.html',
    '..\\index.html',
    '/index.html',
    '.secret',
    'custom-ui-builtins.mjs',
    'toString',
  ])
    assert.equal(await service.resolveAssetPath('pisper-island', path), null)
  assert.throws(() => service.renewView(view.id, 'another-owner'), /过期/)
  now = 4 * 60_000
  service.renewView(view.id)
  now += 4 * 60_000
  assert.equal((await request()).status, 200)
  now += 6 * 60_000
  assert.equal((await request()).status, 404)
  const fresh = await service.createView('pisper-island')
  service.revokeView(fresh.id)
  assert.equal(service.getView(fresh.id), null)
  const direct = await captureResponse((res, json) =>
    service.serveAsset({ id: 'pisper-island', path: 'index.html', res, json }),
  )
  assert.equal(direct.status, 200)
  assert.match(direct.headers['Content-Security-Policy'], /sandbox allow-scripts/)
})

function mountIsland({
  language = 'en-US',
  readyLocale,
  notifyReject = false,
  audio = false,
  audioDenied = false,
  readyPermissions = ['notify'],
} = {}) {
  const html = BUILTIN_CUSTOM_UI_COMPONENTS.find((item) => item.id === 'pisper-island').assets[
    'index.html'
  ]
  const nodes = inspectHtml(html)
  const elements = new Map()
  const parents = new Map()
  const documentEvents = new Map()
  const document = {
    getElementById: (id) => elements.get(id),
    documentElement: {},
    activeElement: null,
    visibilityState: 'visible',
    hidden: false,
    addEventListener(event, callback) {
      const listeners = documentEvents.get(event) ?? []
      documentEvents.set(event, [...listeners, callback])
    },
  }
  for (const node of nodes) {
    const id = node.attrs?.find((attr) => attr.name === 'id')?.value
    if (!id) continue
    const attributes = Object.fromEntries(node.attrs.map((attr) => [attr.name, attr.value]))
    let parent = node.parentNode
    while (parent) {
      const parentId = parent.attrs?.find((attr) => attr.name === 'id')?.value
      if (parentId) {
        parents.set(id, parentId)
        break
      }
      parent = parent.parentNode
    }
    const events = new Map()
    elements.set(id, {
      attributes,
      events,
      style: {},
      dataset: {},
      textContent: '',
      hidden: Object.hasOwn(attributes, 'hidden'),
      disabled: Object.hasOwn(attributes, 'disabled'),
      checked: Object.hasOwn(attributes, 'checked'),
      value: attributes.value ?? '',
      selectionStart: 0,
      selectionEnd: 0,
      setAttribute(key, value) {
        attributes[key] = value
      },
      getAttribute(key) {
        return attributes[key] ?? null
      },
      focus() {
        document.activeElement = this
      },
      select() {
        this.selectionStart = 0
        this.selectionEnd = this.value.length
      },
      addEventListener(event, callback) {
        const listeners = events.get(event) ?? []
        events.set(event, [...listeners, callback])
      },
    })
  }
  let now = Date.UTC(2026, 0, 1, 9, 0)
  const timers = new Map()
  const events = new Map()
  let timerId = 0
  let onTheme
  const notifications = []
  const audioContexts = []
  const tones = []
  class FakeAudioContext {
    constructor() {
      this.state = 'suspended'
      this.currentTime = 0
      this.destination = {}
      audioContexts.push(this)
    }
    resume() {
      if (audioDenied) return Promise.reject(new Error('Audio blocked'))
      this.state = 'running'
      return Promise.resolve()
    }
    close() {
      this.state = 'closed'
      return Promise.resolve()
    }
    createOscillator() {
      return {
        frequency: {
          setValueAtTime() {},
          exponentialRampToValueAtTime() {},
          linearRampToValueAtTime() {},
        },
        connect() {},
        disconnect() {},
        start() {
          tones.push(this)
        },
        stop() {
          queueMicrotask(() => this.onended?.())
        },
        addEventListener(event, callback) {
          if (event === 'ended') this.onended = callback
        },
      }
    }
    createGain() {
      return {
        gain: {
          setValueAtTime() {},
          exponentialRampToValueAtTime() {},
          linearRampToValueAtTime() {},
          cancelScheduledValues() {},
        },
        connect() {},
        disconnect() {},
      }
    }
  }
  class Clock extends Date {
    constructor(...args) {
      super(...(args.length ? args : [now]))
    }
    static now() {
      return now
    }
  }
  const window = {
    addEventListener(event, callback) {
      const listeners = events.get(event) ?? []
      events.set(event, [...listeners, callback])
    },
    pisper: {
      ready: async () => ({ locale: readyLocale, component: { permissions: readyPermissions } }),
      notify: async (message) => {
        notifications.push(message)
        if (notifyReject) throw new Error('Notifications unavailable')
      },
      onThemeChanged(callback) {
        onTheme = callback
        return () => {}
      },
    },
  }
  if (audio) window.AudioContext = FakeAudioContext
  const scripts = nodes.filter(
    (node) => node.tagName === 'script' && !node.attrs.some((attr) => attr.name === 'src'),
  )
  for (const script of scripts) {
    runInNewContext(script.childNodes.map((node) => node.value ?? '').join(''), {
      document,
      window,
      navigator: { language },
      Date: Clock,
      ...(audio ? { AudioContext: FakeAudioContext } : {}),
      setInterval(callback) {
        const id = ++timerId
        timers.set(id, callback)
        return id
      },
      clearInterval(id) {
        timers.delete(id)
      },
    })
  }
  const emit = (id, type, extra = {}) => {
    const target = elements.get(id)
    assert.ok(target, `missing island element: ${id}`)
    let stopped = false
    const event = {
      type,
      target,
      currentTarget: target,
      defaultPrevented: false,
      preventDefault() {
        this.defaultPrevented = true
      },
      stopPropagation() {
        stopped = true
      },
      ...extra,
    }
    let currentId = id
    while (currentId && !stopped) {
      const current = elements.get(currentId)
      event.currentTarget = current
      for (const callback of current.events.get(type) ?? []) callback.call(current, event)
      currentId = parents.get(currentId)
    }
    if (!stopped) {
      event.currentTarget = document
      for (const callback of documentEvents.get(type) ?? []) callback.call(document, event)
    }
    if (!stopped) {
      event.currentTarget = window
      for (const callback of events.get(type) ?? []) callback.call(window, event)
    }
    return event
  }
  return {
    elements,
    document,
    timers,
    notifications,
    audioContexts,
    tones,
    click(id) {
      const element = elements.get(id)
      assert.ok(element, `missing island element: ${id}`)
      if (element.disabled) return
      element.focus()
      return emit(id, 'click')
    },
    change(id, value) {
      elements.get(id).value = String(value)
      return emit(id, 'change')
    },
    input(id, value) {
      elements.get(id).value = String(value)
      return emit(id, 'input')
    },
    key(id, key) {
      return emit(id, 'keydown', { key })
    },
    advance(ms) {
      now += ms
      for (const callback of timers.values()) callback()
    },
    elapse(ms) {
      now += ms
    },
    dispatch(event) {
      for (const callback of events.get(event) ?? []) callback()
    },
    visibility(state) {
      document.visibilityState = state
      document.hidden = state === 'hidden'
      for (const callback of documentEvents.get('visibilitychange') ?? []) callback()
    },
    theme(locale) {
      onTheme({ locale })
    },
  }
}

test('island timer starts, pauses, resumes, completes and resets using elapsed time', () => {
  const app = mountIsland()
  const read = (id) => app.elements.get(id)
  assert.equal(read('remaining').textContent, '25:00')
  assert.equal(read('reset').disabled, true)
  app.click('toggle')
  assert.equal(read('island').dataset.state, 'running')
  app.advance(61_000)
  assert.equal(read('remaining').textContent, '23:59')
  app.click('toggle')
  assert.equal(read('island').dataset.state, 'paused')
  app.advance(120_000)
  assert.equal(read('remaining').textContent, '23:59')
  app.click('toggle')
  app.advance(24 * 60_000)
  assert.equal(read('remaining').textContent, '00:00')
  assert.equal(read('island').dataset.state, 'complete')
  assert.equal(read('announcement').textContent, 'Focus session complete')
  app.click('toggle')
  assert.equal(read('remaining').textContent, '25:00')
  assert.equal(read('island').dataset.state, 'running')
  app.click('reset')
  assert.equal(read('island').dataset.state, 'idle')
  assert.equal(read('reset').disabled, true)
})

test('island follows bridge locale without restarting the timer and releases its page timer', async () => {
  const app = mountIsland({ language: 'en-US', readyLocale: 'zh-CN' })
  await Promise.resolve()
  assert.equal(app.document.documentElement.lang, 'zh-CN')
  assert.equal(app.elements.get('toggle').attributes['aria-label'], '开始专注')
  app.click('toggle')
  app.advance(12_000)
  app.theme('en-US')
  assert.equal(app.elements.get('label').textContent, 'Focus')
  assert.equal(app.elements.get('remaining').textContent, '24:48')
  app.theme('invalid_locale!')
  assert.equal(app.elements.get('remaining').textContent, '24:48')
  app.dispatch('pagehide')
  assert.equal(app.timers.size, 0)
  app.advance(100_000)
  app.dispatch('pageshow')
  assert.equal(app.timers.size, 1)
  assert.equal(app.elements.get('remaining').textContent, '23:08')
  app.dispatch('pageshow')
  assert.equal(app.timers.size, 1)
  app.dispatch('pagehide')
  assert.equal(app.timers.size, 0)
})

test('duration settings explain the timer, focus its input and apply presets without a sandbox form submission', () => {
  const app = mountIsland()
  const read = (id) => app.elements.get(id)
  assert.equal(read('duration-settings').hidden, true)
  assert.equal(read('apply-duration').attributes.type, 'button')
  app.click('edit-duration')
  assert.equal(read('duration-settings').hidden, false)
  assert.equal(app.document.activeElement, read('duration-input'))
  assert.equal(read('duration-input').value, '25')
  assert.equal(read('duration-input').selectionStart, 0)
  assert.equal(read('duration-input').selectionEnd, 2)
  assert.ok(read('settings-hint').textContent.trim())
  for (const minutes of [15, 25, 45]) {
    app.change('duration-presets', minutes)
    assert.equal(read('duration-input').value, String(minutes))
    assert.equal(read('remaining').textContent, '25:00')
  }
  assert.equal(app.click('apply-duration').defaultPrevented, true)
  assert.equal(read('remaining').textContent, '45:00')
  assert.equal(read('island').dataset.state, 'idle')
  assert.equal(read('duration-settings').hidden, true)
  assert.equal(app.document.activeElement, read('edit-duration'))
})

test('saving a custom duration stops the current round and reset retains the chosen duration', () => {
  const app = mountIsland()
  const read = (id) => app.elements.get(id)
  app.click('toggle')
  app.advance(60_000)
  app.click('edit-duration')
  const runningHint = read('settings-hint').textContent
  assert.ok(runningHint.trim())
  app.input('duration-input', '12')
  assert.equal(app.key('duration-input', 'Enter').defaultPrevented, true)
  assert.equal(read('island').dataset.state, 'idle')
  assert.equal(read('remaining').textContent, '12:00')
  assert.equal(read('reset').disabled, true)
  app.advance(60_000)
  assert.equal(read('remaining').textContent, '12:00')
  app.click('toggle')
  app.advance(90_000)
  assert.equal(read('remaining').textContent, '10:30')
  app.click('reset')
  assert.equal(read('remaining').textContent, '12:00')
  assert.equal(read('island').dataset.state, 'idle')
  app.click('edit-duration')
  assert.equal(read('duration-input').value, '12')
  assert.notEqual(read('settings-hint').textContent, runningHint)
})

for (const minutes of [1, 180]) {
  test(`duration settings accept ${minutes} minutes at the supported boundary`, () => {
    const app = mountIsland()
    app.click('edit-duration')
    app.input('duration-input', minutes)
    app.click('apply-duration')
    assert.equal(
      app.elements.get('remaining').textContent,
      `${String(minutes).padStart(2, '0')}:00`,
    )
    assert.equal(app.elements.get('duration-settings').hidden, true)
  })
}

for (const minutes of ['', ' ', '0', '181', '1.5', '-1', 'abc']) {
  test(`invalid duration ${JSON.stringify(minutes)} keeps editing and preserves the running timer`, () => {
    const app = mountIsland()
    const read = (id) => app.elements.get(id)
    app.click('toggle')
    app.advance(5_000)
    app.click('edit-duration')
    app.input('duration-input', minutes)
    assert.equal(app.click('apply-duration').defaultPrevented, true)
    assert.equal(read('duration-settings').hidden, false)
    assert.ok(read('settings-error').textContent.trim())
    assert.equal(read('island').dataset.state, 'running')
    assert.equal(read('remaining').textContent, '24:55')
    app.advance(1_000)
    assert.equal(read('remaining').textContent, '24:54')
  })
}

for (const close of ['cancel', 'Escape']) {
  test(`${close} discards duration changes and restores focus without extending the deadline`, () => {
    const app = mountIsland()
    const read = (id) => app.elements.get(id)
    app.click('toggle')
    app.advance(1_000)
    app.click('edit-duration')
    app.input('duration-input', '10')
    app.advance(31_000)
    if (close === 'cancel') app.click('cancel-duration')
    else assert.equal(app.key('duration-input', 'Escape').defaultPrevented, true)
    assert.equal(read('duration-settings').hidden, true)
    assert.equal(read('island').dataset.state, 'running')
    assert.equal(read('remaining').textContent, '24:28')
    assert.equal(app.document.activeElement, read('edit-duration'))
    app.advance(5_000)
    assert.equal(read('remaining').textContent, '24:23')
    app.click('edit-duration')
    assert.equal(read('duration-input').value, '25')
  })
}

test('duration help and validation switch language while preserving the draft and countdown', async () => {
  const app = mountIsland({ readyLocale: 'en-US' })
  await Promise.resolve()
  const read = (id) => app.elements.get(id)
  app.click('toggle')
  app.advance(10_000)
  app.click('edit-duration')
  app.input('duration-input', '181')
  app.click('apply-duration')
  const help = read('settings-hint').textContent
  const error = read('settings-error').textContent
  app.theme('zh-CN')
  assert.notEqual(read('settings-hint').textContent, help)
  assert.notEqual(read('settings-error').textContent, error)
  assert.match(read('settings-hint').textContent, /[\u4e00-\u9fff]/)
  assert.equal(read('duration-input').value, '181')
  assert.equal(read('duration-settings').hidden, false)
  assert.equal(read('remaining').textContent, '24:50')
  app.theme('en-US')
  assert.equal(read('settings-hint').textContent, help)
  assert.equal(read('settings-error').textContent, error)
})

test('each completed round sends one localized notification and a new round can notify again', async () => {
  const app = mountIsland({ readyLocale: 'zh-CN' })
  await Promise.resolve()
  app.click('edit-duration')
  app.input('duration-input', '1')
  app.click('apply-duration')
  app.click('toggle')
  app.advance(60_000)
  await setImmediate()
  assert.equal(app.notifications.length, 1)
  assert.match(app.notifications[0], /1/)
  assert.match(app.notifications[0], /[\u4e00-\u9fff]/)
  const first = app.notifications[0]
  assert.equal(app.elements.get('island').dataset.state, 'complete')
  app.advance(120_000)
  app.visibility('hidden')
  app.visibility('visible')
  app.theme('en-US')
  assert.equal(app.notifications.length, 1)
  app.click('toggle')
  app.advance(60_000)
  await setImmediate()
  assert.equal(app.notifications.length, 2)
  assert.notEqual(app.notifications[1], first)
  assert.match(app.notifications[1], /1/)
})

test('returning to a visible page completes a throttled timer once according to its deadline', async () => {
  const app = mountIsland()
  await Promise.resolve()
  app.click('toggle')
  app.visibility('hidden')
  app.elapse(26 * 60_000)
  assert.equal(app.notifications.length, 0)
  app.visibility('visible')
  await setImmediate()
  assert.equal(app.elements.get('remaining').textContent, '00:00')
  assert.equal(app.elements.get('island').dataset.state, 'complete')
  assert.equal(app.notifications.length, 1)
  app.visibility('visible')
  app.advance(1_000)
  assert.equal(app.notifications.length, 1)
})

test('sound is enabled by default and a muted round still posts its completion notification', async () => {
  const audible = mountIsland({ audio: true })
  await Promise.resolve()
  assert.equal(audible.elements.get('sound-enabled').checked, true)
  audible.click('toggle')
  await setImmediate()
  const before = audible.tones.length
  audible.advance(25 * 60_000)
  await setImmediate()
  assert.ok(audible.tones.length > before)
  assert.equal(audible.notifications.length, 1)

  const muted = mountIsland({ audio: true })
  await Promise.resolve()
  muted.click('edit-duration')
  muted.elements.get('sound-enabled').checked = false
  muted.click('apply-duration')
  muted.click('toggle')
  await setImmediate()
  const mutedBefore = muted.tones.length
  muted.advance(25 * 60_000)
  await setImmediate()
  assert.equal(muted.tones.length, mutedBefore)
  assert.equal(muted.notifications.length, 1)
})

test('notification or audio rejection keeps visible completion and does not cause unhandled rejections', async () => {
  const app = mountIsland({ audio: true, audioDenied: true, notifyReject: true })
  await Promise.resolve()
  app.click('toggle')
  await setImmediate()
  app.advance(25 * 60_000)
  await setImmediate()
  assert.equal(app.elements.get('island').dataset.state, 'complete')
  assert.equal(app.elements.get('remaining').textContent, '00:00')
  assert.equal(app.notifications.length, 1)
  app.advance(10_000)
  await setImmediate()
  assert.equal(app.notifications.length, 1)
})

test('a layout preview without notify permission cannot play sounds or post completion notifications', async () => {
  const app = mountIsland({ audio: true, readyPermissions: [] })
  await Promise.resolve()
  app.click('toggle')
  app.advance(25 * 60_000)
  await setImmediate()
  assert.equal(app.elements.get('island').dataset.state, 'complete')
  assert.equal(app.notifications.length, 0)
  assert.equal(app.tones.length, 0)
  assert.equal(app.audioContexts.length, 0)
})
