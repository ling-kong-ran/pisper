import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CHAT_LAYOUT_MAX_BYTES,
  CHAT_LAYOUT_PRESETS,
  ChatLayoutValidationError,
  DEFAULT_CHAT_LAYOUT,
  parseChatLayout,
  parseChatLayoutJson,
  serializeChatLayout,
} from '../../src/features/chat/layout/chat-layout.ts'

const fresh = () => structuredClone(DEFAULT_CHAT_LAYOUT)

function rejects(value, code, path) {
  assert.throws(
    () => parseChatLayout(value),
    (error) => {
      assert.ok(error instanceof ChatLayoutValidationError)
      assert.equal(error.code, code)
      if (path) assert.equal(error.path, path)
      return true
    },
  )
}

test('built-in layouts round trip through the same contract as imported layouts', () => {
  assert.deepEqual(
    CHAT_LAYOUT_PRESETS.map((preset) => preset.id),
    ['default', 'focus', 'workbench', 'studio'],
  )
  for (const { template } of CHAT_LAYOUT_PRESETS) {
    const parsed = parseChatLayoutJson(serializeChatLayout(template))
    assert.deepEqual(parsed, template)
    assert.notEqual(parsed, template)
    assert.notEqual(parsed.desktop, template.desktop)
    assert.notEqual(parsed.mobile, template.mobile)
  }
  assert.throws(() => {
    DEFAULT_CHAT_LAYOUT.desktop.canvas.children[0].css = 'color: red'
  }, TypeError)
  assert.throws(() => {
    DEFAULT_CHAT_LAYOUT.mobile.canvas.children.push({})
  }, TypeError)
})

test('default layout keeps current widths, inherited typography and automatic completion panels', () => {
  const { desktop, mobile } = DEFAULT_CHAT_LAYOUT
  assert.equal(desktop.contentWidth, 1040)
  assert.equal(desktop.contextWidth, 360)
  assert.equal(desktop.navigationWidth, 236)
  assert.equal(desktop.navigationCollapsed, null)
  for (const viewport of [desktop, mobile]) {
    assert.equal(viewport.composerPosition, 'bottom')
    assert.equal(viewport.fontFamily, 'inherit')
    assert.equal(viewport.fontSize, null)
    assert.equal(viewport.openContextOnCompletion, true)
  }
  const focus = CHAT_LAYOUT_PRESETS.find((preset) => preset.id === 'focus')
  assert.equal(focus.template.mobile.openContextOnCompletion, false)
})

test('desktop and mobile appearance settings are independent', () => {
  const template = fresh()
  template.desktop.composerPosition = 'top'
  template.desktop.fontSize = 20
  template.mobile.fontSize = 13
  template.mobile.density = 'compact'
  const parsed = parseChatLayout(template)
  assert.equal(parsed.desktop.composerPosition, 'top')
  assert.equal(parsed.mobile.composerPosition, 'bottom')
  assert.equal(parsed.desktop.fontSize, 20)
  assert.equal(parsed.mobile.fontSize, 13)
  parsed.mobile.fontSize = 15
  assert.equal(template.mobile.fontSize, 13)
})

test('layout names are trimmed and limited to 80 visible Unicode characters', () => {
  assert.equal(parseChatLayout({ ...fresh(), name: '  我的布局  ' }).name, '我的布局')
  assert.equal(parseChatLayout({ ...fresh(), name: '🌟'.repeat(80) }).name, '🌟'.repeat(80))
  for (const name of ['', '   ', 'a'.repeat(81), '布局\n秘密', '\u0000title'])
    rejects({ ...fresh(), name }, 'invalid_value', 'name')
  rejects({ ...fresh(), name: 42 }, 'invalid_type', 'name')
})

test('unknown fields cannot add CSS, scripts, URLs or a persistent mobile sidebar', () => {
  for (const field of ['css', 'script', 'html', 'url']) {
    const value = { ...fresh(), [field]: 'secret-user-input' }
    assert.throws(
      () => parseChatLayout(value),
      (error) => {
        assert.equal(error.code, 'unknown_field')
        assert.equal(error.path, 'layout')
        assert.ok(!error.message.includes('secret-user-input'))
        return true
      },
    )
  }
  const mobile = fresh()
  mobile.mobile.contextSide = 'left'
  rejects(mobile, 'unknown_field', 'mobile')
  const desktop = fresh()
  desktop.desktop.style = { color: 'red' }
  rejects(desktop, 'unknown_field', 'desktop')
  rejects(
    JSON.parse(
      serializeChatLayout(fresh()).replace('"version": 2', '"__proto__": {}, "version": 2'),
    ),
    'unknown_field',
  )
})

test('every template field is required and enum values are closed', () => {
  const missing = fresh()
  delete missing.desktop.showUsage
  rejects(missing, 'missing_field', 'desktop.showUsage')
  for (const [field, value] of [
    ['composerPosition', 'floating'],
    ['fontFamily', 'url(remote-font)'],
    ['messageStyle', 'html'],
    ['density', 'tiny'],
    ['contextVisibility', 'always'],
    ['contextSide', 'bottom'],
    ['navigationSide', 'hidden'],
  ]) {
    const template = fresh()
    template.desktop[field] = value
    rejects(template, 'invalid_value', `desktop.${field}`)
  }
  rejects({ ...fresh(), accent: '#abc' }, 'invalid_value', 'accent')
})

test('dimension and font limits accept boundaries and reject non-numbers or overflow', () => {
  for (const [field, minimum, maximum] of [
    ['contentWidth', 600, 1600],
    ['fontSize', 13, 20],
    ['navigationWidth', 200, 320],
    ['contextWidth', 280, 720],
  ]) {
    for (const value of [minimum, maximum]) {
      const template = fresh()
      template.desktop[field] = value
      assert.equal(parseChatLayout(template).desktop[field], value)
    }
    for (const value of [minimum - 1, maximum + 1]) {
      const template = fresh()
      template.desktop[field] = value
      rejects(template, 'out_of_range', `desktop.${field}`)
    }
    for (const value of [String(minimum), undefined, NaN, Infinity]) {
      const template = fresh()
      template.desktop[field] = value
      rejects(template, 'invalid_type', `desktop.${field}`)
    }
  }
  const invalidBoolean = fresh()
  invalidBoolean.mobile.showUsage = 'false'
  rejects(invalidBoolean, 'invalid_type', 'mobile.showUsage')
})

test('unknown versions and non-data objects are rejected without invoking getters', () => {
  for (const version of [0, 3, '1', '2', null])
    rejects({ ...fresh(), version }, 'unsupported_version', 'version')
  for (const value of [null, [], 'layout', 1, new Date()]) rejects(value, 'invalid_type')
  let invoked = false
  const template = fresh()
  Object.defineProperty(template, 'desktop', {
    get() {
      invoked = true
      throw new Error('Must not execute')
    },
  })
  rejects(template, 'invalid_type', 'layout.desktop')
  assert.equal(invoked, false)
})

test('v1 templates migrate to independent v2 canvases while retaining appearance and ordering', () => {
  const legacy = fresh()
  legacy.version = 1
  delete legacy.desktop.canvas
  delete legacy.mobile.canvas
  legacy.desktop.composerPosition = 'top'
  legacy.mobile.composerPosition = 'bottom'
  legacy.desktop.contentWidth = 1400
  const parsed = parseChatLayout(legacy)
  assert.equal(parsed.version, 2)
  assert.equal(parsed.desktop.contentWidth, 1400)
  assert.deepEqual(
    parsed.desktop.canvas.children.map((node) => node.kind),
    ['header', 'composer', 'messages'],
  )
  assert.deepEqual(
    parsed.mobile.canvas.children.map((node) => node.kind),
    ['header', 'messages', 'composer'],
  )
  assert.notEqual(parsed.desktop.canvas, parsed.mobile.canvas)
  assert.equal('canvas' in legacy.desktop, false)
  assert.deepEqual(parseChatLayoutJson(serializeChatLayout(parsed)), parsed)
})

test('v2 requires a valid independent canvas on each viewport and v1 does not silently accept one', () => {
  const missing = fresh()
  delete missing.mobile.canvas
  rejects(missing, 'missing_field', 'mobile.canvas')
  const invalid = fresh()
  invalid.mobile.canvas.children = []
  rejects(invalid, 'invalid_canvas')
  rejects({ ...fresh(), version: 1 }, 'unknown_field', 'desktop')
  const unsafe = fresh()
  unsafe.desktop.canvas.css = 'background-image: url(https://example.com)'
  rejects(unsafe, 'invalid_css')
})

test('JSON import limits bytes before parsing and reports stable errors', () => {
  for (const input of ['{', 'undefined', '<script>'])
    assert.throws(() => parseChatLayoutJson(input), { code: 'invalid_json' })
  const json = serializeChatLayout(fresh())
  const boundary = json + ' '.repeat(CHAT_LAYOUT_MAX_BYTES - new TextEncoder().encode(json).length)
  assert.deepEqual(parseChatLayoutJson(boundary), fresh())
  assert.throws(() => parseChatLayoutJson(`${boundary} `), { code: 'too_large' })
  assert.throws(() => parseChatLayoutJson('中'.repeat(22000)), { code: 'too_large' })
})
