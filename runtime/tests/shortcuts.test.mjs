import assert from 'node:assert/strict'
import test from 'node:test'
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  ShortcutValidationError,
  findShortcutConflict,
  formatShortcut,
  matchesShortcut,
  normalizeShortcutBindings,
  shortcutFromEvent,
  validateShortcutBindings,
} from '../../shared/shortcuts.mjs'

function bindingsWith(action, binding) {
  return { ...DEFAULT_SHORTCUTS, [action]: binding }
}

function event(code, modifiers = {}) {
  return { code, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...modifiers }
}

function invalid(input, code = 'invalid') {
  assert.throws(
    () => validateShortcutBindings(input),
    (error) => error instanceof ShortcutValidationError && error.code === code,
  )
}

test('shortcut defaults preserve the action order and are valid immutable templates', () => {
  assert.deepEqual(SHORTCUT_ACTIONS, [
    'commandPalette',
    'primaryAction',
    'focusSearch',
    'toggleSidebar',
    'toggleTerminal',
    'openSettings',
    'focusComposer',
    'sendMessage',
    'voiceInput',
  ])
  assert.deepEqual(Object.values(DEFAULT_SHORTCUTS), [
    'Mod+KeyK',
    'Mod+KeyN',
    'Slash',
    'Mod+KeyB',
    'Mod+Backquote',
    'Mod+Comma',
    'Mod+Shift+KeyL',
    'Enter',
    'F8',
  ])
  assert.deepEqual(validateShortcutBindings(DEFAULT_SHORTCUTS), DEFAULT_SHORTCUTS)
  assert.notEqual(validateShortcutBindings(DEFAULT_SHORTCUTS), DEFAULT_SHORTCUTS)
  assert.ok(Object.isFrozen(DEFAULT_SHORTCUTS))
  assert.ok(Object.isFrozen(SHORTCUT_ACTIONS))
})

test('old voice default migrates to F8 without overriding custom, disabled or conflicting bindings', () => {
  const old = { ...DEFAULT_SHORTCUTS, voiceInput: 'Mod+Shift+Space', sendMessage: 'Mod+Enter' }
  assert.equal(normalizeShortcutBindings(old).voiceInput, 'F8')
  assert.equal(normalizeShortcutBindings(old).sendMessage, 'Mod+Enter')
  assert.equal(old.voiceInput, 'Mod+Shift+Space')
  for (const binding of [null, 'F9', 'Ctrl+Shift+KeyH']) {
    assert.equal(normalizeShortcutBindings({ ...old, voiceInput: binding }).voiceInput, binding)
  }
  assert.deepEqual(normalizeShortcutBindings({ ...old, commandPalette: 'F8' }), {
    ...old,
    commandPalette: 'F8',
  })
})

test('persisted junk resets the complete map without mutating shared defaults', () => {
  const incomplete = { ...DEFAULT_SHORTCUTS }
  delete incomplete.voiceInput
  const unknown = { ...DEFAULT_SHORTCUTS, unknown: null }
  const junk = [
    undefined,
    null,
    false,
    42,
    'Mod+KeyK',
    [],
    {},
    new Date(),
    incomplete,
    unknown,
    { ...DEFAULT_SHORTCUTS, [Symbol('unknown')]: null },
    Object.create(DEFAULT_SHORTCUTS),
    bindingsWith('voiceInput', 1),
    bindingsWith('voiceInput', undefined),
    bindingsWith('voiceInput', {}),
    bindingsWith('voiceInput', ['F1']),
    { ...DEFAULT_SHORTCUTS, commandPalette: 'F1', voiceInput: 'invalid' },
    bindingsWith('voiceInput', 'Ctrl+KeyK'),
  ]
  for (const input of junk) {
    assert.throws(() => validateShortcutBindings(input), ShortcutValidationError)
    const normalized = normalizeShortcutBindings(input)
    assert.deepEqual(normalized, DEFAULT_SHORTCUTS)
    assert.notEqual(normalized, DEFAULT_SHORTCUTS)
    normalized.sendMessage = null
    assert.equal(DEFAULT_SHORTCUTS.sendMessage, 'Enter')
  }
})

test('valid persisted maps support disabled actions, round trips, and restoring defaults', () => {
  const input = {
    ...DEFAULT_SHORTCUTS,
    commandPalette: 'F2',
    sendMessage: 'Mod+Enter',
    voiceInput: null,
  }
  const normalized = normalizeShortcutBindings(JSON.parse(JSON.stringify(input)))
  assert.deepEqual(normalized, input)
  assert.notEqual(normalized, input)
  assert.equal(
    matchesShortcut(event('Space', { ctrlKey: true, shiftKey: true }), normalized.voiceInput),
    false,
  )
  assert.deepEqual(normalizeShortcutBindings(DEFAULT_SHORTCUTS), DEFAULT_SHORTCUTS)
  const disabled = Object.fromEntries(SHORTCUT_ACTIONS.map((action) => [action, null]))
  assert.deepEqual(validateShortcutBindings(disabled), disabled)
  assert.deepEqual(validateShortcutBindings(Object.assign(Object.create(null), input)), input)
})

test('duplicate detection accounts for Mod aliases on both platforms', () => {
  for (const candidate of ['Mod+KeyK', 'Ctrl+KeyK', 'Meta+KeyK']) {
    invalid(bindingsWith('voiceInput', candidate), 'conflict')
    assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'voiceInput', candidate), 'commandPalette')
  }
  for (const primary of ['Ctrl', 'Meta']) {
    const bindings = bindingsWith('commandPalette', `${primary}+KeyK`)
    assert.equal(findShortcutConflict(bindings, 'voiceInput', 'Mod+KeyK'), 'commandPalette')
  }
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'commandPalette', 'Mod+KeyK'), null)
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'voiceInput', null), null)
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'voiceInput', 'malformed'), null)
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'voiceInput', 'Mod+Shift+KeyK'), null)
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'voiceInput', 'Mod+Alt+KeyK'), null)
  const distinct = { ...DEFAULT_SHORTCUTS, commandPalette: 'Ctrl+KeyK', voiceInput: 'Meta+KeyK' }
  assert.deepEqual(validateShortcutBindings(distinct), distinct)
  assert.equal(
    findShortcutConflict(bindingsWith('commandPalette', null), 'voiceInput', 'Mod+KeyK'),
    null,
  )
})

test('validation errors expose the conflicting actions', () => {
  assert.throws(
    () => validateShortcutBindings(bindingsWith('voiceInput', 'Mod+KeyK')),
    (error) => {
      assert.equal(error.name, 'ShortcutValidationError')
      assert.equal(error.code, 'conflict')
      assert.equal(error.action, 'commandPalette')
      assert.equal(error.conflictingAction, 'voiceInput')
      assert.match(error.message, /commandPalette.*voiceInput/)
      return true
    },
  )
})

test('malformed bindings and modifier-only bindings are rejected', () => {
  for (const binding of [
    '',
    ' ',
    'Mod',
    'Control',
    'ShiftLeft',
    'Unidentified',
    'Keyk',
    'Mod+K',
    'Cmd+KeyK',
    'Control+KeyK',
    'mod+KeyK',
    'Mod+KeyK ',
    ' Mod+KeyK',
    'Mod++KeyK',
    'Mod+Mod+KeyK',
    'Shift+Mod+KeyK',
    'Alt+Ctrl+KeyK',
    'Mod+Ctrl+KeyK',
    'Ctrl+Meta+KeyK',
    'AltGraph+KeyK',
    'Mod+KeyK+Shift',
    'Mod+F13',
    'F0',
    'F01',
    'KeyAA',
  ]) {
    invalid(bindingsWith('voiceInput', binding))
  }
})

test('bare typing and navigation keys are reserved with action-specific Enter and Slash', () => {
  for (const binding of [
    'KeyK',
    'Digit1',
    'Space',
    'Backquote',
    'Comma',
    'Escape',
    'Tab',
    'ArrowUp',
    'ArrowDown',
    'ArrowLeft',
    'ArrowRight',
    'Home',
    'End',
    'PageUp',
    'PageDown',
    'Backspace',
    'Delete',
    'Insert',
    'Shift+KeyK',
    'Shift+Slash',
    'Shift+Enter',
    'Mod+Escape',
    'Mod+Shift+Escape',
    'Alt+Tab',
    'Mod+ArrowLeft',
  ]) {
    invalid(bindingsWith('voiceInput', binding))
    assert.equal(matchesShortcut(event(binding), binding), false)
  }
  invalid(bindingsWith('voiceInput', 'Enter'))
  invalid(bindingsWith('voiceInput', 'Slash'))
  for (const action of SHORTCUT_ACTIONS) {
    invalid(bindingsWith(action, 'Shift+Enter'))
    for (const binding of ['F1', 'F12', 'Alt+KeyK', 'Mod+Digit1', 'Ctrl+Alt+Period']) {
      assert.equal(validateShortcutBindings(bindingsWith(action, binding))[action], binding)
    }
  }
  assert.equal(
    validateShortcutBindings(bindingsWith('sendMessage', 'Mod+Enter')).sendMessage,
    'Mod+Enter',
  )
})

test('OS and browser editing/navigation chords are reserved on both platforms', () => {
  for (const primary of ['Mod', 'Ctrl', 'Meta']) {
    for (const key of ['A', 'C', 'F', 'L', 'O', 'P', 'Q', 'R', 'S', 'T', 'V', 'W', 'X', 'Y', 'Z']) {
      invalid(bindingsWith('voiceInput', `${primary}+Key${key}`))
      if (key !== 'L') invalid(bindingsWith('voiceInput', `${primary}+Shift+Key${key}`))
    }
    invalid(bindingsWith('voiceInput', `${primary}+F4`))
  }
  for (const binding of ['Alt+F4', 'Alt+Shift+F4', 'Alt+Space', 'Mod+Space', 'Meta+Space']) {
    invalid(bindingsWith('voiceInput', binding))
  }
  assert.equal(shortcutFromEvent(event('KeyW', { ctrlKey: true })), null)
  assert.equal(shortcutFromEvent(event('Enter', { shiftKey: true })), null)
  assert.equal(shortcutFromEvent(event('KeyN', { ctrlKey: true })), 'Mod+KeyN')
})

test('recording uses physical codes and canonical primary platform modifiers', () => {
  assert.equal(shortcutFromEvent(event('KeyK', { key: 'x', ctrlKey: true })), 'Mod+KeyK')
  assert.equal(shortcutFromEvent(event('KeyK', { metaKey: true }), true), 'Mod+KeyK')
  assert.equal(shortcutFromEvent(event('KeyK', { ctrlKey: true }), true), 'Ctrl+KeyK')
  assert.equal(shortcutFromEvent(event('KeyK', { metaKey: true }), false), 'Meta+KeyK')
  assert.equal(
    shortcutFromEvent(event('Period', { ctrlKey: true, altKey: true, shiftKey: true })),
    'Mod+Alt+Shift+Period',
  )
  assert.equal(shortcutFromEvent(event('F1')), 'F1')
  assert.equal(shortcutFromEvent(event('Slash')), 'Slash')
  assert.equal(shortcutFromEvent(event('Enter')), 'Enter')
  assert.equal(shortcutFromEvent(event('KeyK', { ctrlKey: true, metaKey: true })), null)
})

test('matching resolves Mod by platform and requires exact modifiers', () => {
  assert.equal(matchesShortcut(event('KeyK', { ctrlKey: true }), 'Mod+KeyK', false), true)
  assert.equal(matchesShortcut(event('KeyK', { metaKey: true }), 'Mod+KeyK', true), true)
  assert.equal(matchesShortcut(event('KeyK', { ctrlKey: true }), 'Mod+KeyK', true), false)
  assert.equal(matchesShortcut(event('KeyK', { metaKey: true }), 'Mod+KeyK', false), false)
  assert.equal(matchesShortcut(event('KeyK', { ctrlKey: true }), 'Ctrl+KeyK', true), true)
  assert.equal(matchesShortcut(event('KeyK', { metaKey: true }), 'Meta+KeyK', false), true)
  for (const modifier of ['shiftKey', 'altKey', 'metaKey']) {
    assert.equal(
      matchesShortcut(event('KeyK', { ctrlKey: true, [modifier]: true }), 'Mod+KeyK'),
      false,
    )
  }
  assert.equal(matchesShortcut(event('KeyK'), 'Mod+KeyK'), false)
  assert.equal(matchesShortcut(event('KeyN', { ctrlKey: true }), 'Mod+KeyK'), false)
  assert.equal(matchesShortcut(event('Enter'), 'Enter'), true)
  assert.equal(matchesShortcut(event('Enter', { shiftKey: true }), 'Enter'), false)
  assert.equal(
    matchesShortcut(event('KeyK', { ctrlKey: true, shiftKey: true }), 'Mod+Shift+KeyK'),
    true,
  )
  assert.equal(matchesShortcut(event('KeyK', { ctrlKey: true }), null), false)
  assert.equal(matchesShortcut(event('KeyK', { ctrlKey: true }), 'bad'), false)
})

test('composition, repeats, prevented events, and AltGraph are neither recorded nor matched', () => {
  for (const state of [
    { isComposing: true },
    { repeat: true },
    { defaultPrevented: true },
    { keyCode: 229 },
    { key: 'Dead' },
    { key: 'Process' },
    { key: 'Unidentified' },
    { key: 'AltGraph' },
    { getModifierState: (key) => key === 'AltGraph' },
  ]) {
    const input = event('KeyK', { ctrlKey: true, ...state })
    assert.equal(shortcutFromEvent(input), null)
    assert.equal(matchesShortcut(input, 'Mod+KeyK'), false)
  }
  for (const code of [
    'ControlLeft',
    'ControlRight',
    'MetaLeft',
    'AltRight',
    'ShiftLeft',
    'Unidentified',
  ]) {
    assert.equal(shortcutFromEvent(event(code, { ctrlKey: true })), null)
  }
  assert.equal(shortcutFromEvent(event('Unidentified', { key: 'k', ctrlKey: true })), null)
  assert.equal(
    shortcutFromEvent(event('KeyK', { ctrlKey: true, getModifierState: () => false })),
    'Mod+KeyK',
  )
})

test('key fallback is used only when a physical code is missing', () => {
  for (const [key, code, modifiers] of [
    ['k', 'KeyK', { ctrlKey: true }],
    ['K', 'KeyK', { ctrlKey: true, shiftKey: true }],
    ['1', 'Digit1', { altKey: true }],
    ['!', 'Digit1', { altKey: true, shiftKey: true }],
    ['`', 'Backquote', { ctrlKey: true }],
    [',', 'Comma', { ctrlKey: true }],
    ['/', 'Slash', {}],
    ['Enter', 'Enter', {}],
    ['F12', 'F12', {}],
    [' ', 'Space', { ctrlKey: true, shiftKey: true }],
    ['?', 'Slash', { altKey: true, shiftKey: true }],
  ]) {
    const expected = shortcutFromEvent(event(code, modifiers))
    const input = { key, ...modifiers }
    assert.equal(shortcutFromEvent(input), expected)
    assert.equal(matchesShortcut(input, expected), true)
    assert.equal(matchesShortcut({ ...input, code: '' }, expected), true)
  }
  assert.equal(matchesShortcut(event('KeyK', { key: 'x', ctrlKey: true }), 'Mod+KeyK'), true)
  assert.equal(matchesShortcut(event('KeyX', { key: 'k', ctrlKey: true }), 'Mod+KeyK'), false)
  assert.equal(shortcutFromEvent({ key: 'Unidentified' }), null)
  assert.equal(shortcutFromEvent({ key: 'Control', ctrlKey: true }), null)
  assert.equal(shortcutFromEvent({}), null)
})

test('all default bindings can be recorded and matched on both platforms', () => {
  for (const mac of [false, true]) {
    for (const binding of Object.values(DEFAULT_SHORTCUTS)) {
      const parts = binding.split('+')
      const code = parts.pop()
      const input = event(code, {
        ctrlKey: parts.includes('Mod') && !mac,
        metaKey: parts.includes('Mod') && mac,
        shiftKey: parts.includes('Shift'),
      })
      assert.equal(shortcutFromEvent(input, mac), binding)
      assert.equal(matchesShortcut(input, binding, mac), true)
    }
  }
})

test('display uses platform labels and human-readable keys', () => {
  assert.equal(formatShortcut('Mod+Shift+Space'), 'Ctrl + Shift + Space')
  assert.equal(formatShortcut('Mod+Shift+Space', true), 'Cmd + Shift + Space')
  assert.equal(formatShortcut('Ctrl+Alt+KeyK', true), 'Ctrl + Alt + K')
  assert.equal(formatShortcut('Meta+KeyK', false), 'Cmd + K')
  assert.equal(formatShortcut('Mod+Backquote'), 'Ctrl + `')
  assert.equal(formatShortcut('Mod+Comma', true), 'Cmd + ,')
  assert.equal(formatShortcut('Alt+Digit1'), 'Alt + 1')
  assert.equal(formatShortcut('F12'), 'F12')
  assert.equal(formatShortcut('Enter'), 'Enter')
  assert.equal(formatShortcut('Slash'), '/')
  assert.equal(formatShortcut(null), '')
  assert.equal(formatShortcut('bad'), '')
})
