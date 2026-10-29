import assert from 'node:assert/strict'
import test from 'node:test'
import {
  normalizeTypographyPreferences,
  parseFontSizeDraft,
} from '../../src/lib/typography-preferences.ts'
import { uiTextSizes } from '../../src/app/ui-preferences.ts'

test('numeric typography restores legacy scale while keeping code size independent', () => {
  for (const [fontScale, uiFontSize] of [
    ['small', 13.5],
    ['default', 14],
    ['large', 14.5],
  ]) {
    assert.deepEqual(normalizeTypographyPreferences({ fontScale }), {
      uiFontSize,
      codeFontSize: 12,
    })
  }
  assert.deepEqual(
    normalizeTypographyPreferences({ fontScale: 'small', uiFontSize: 16, codeFontSize: 24 }),
    { uiFontSize: 16, codeFontSize: 24 },
  )
})

test('untrusted persisted typography is bounded without coercing strings or nonfinite values', () => {
  assert.deepEqual(normalizeTypographyPreferences(null), { uiFontSize: 14, codeFontSize: 12 })
  assert.deepEqual(normalizeTypographyPreferences({ uiFontSize: 1, codeFontSize: 100 }), {
    uiFontSize: 11,
    codeFontSize: 24,
  })
  for (const invalid of [NaN, Infinity, '16', {}, []]) {
    assert.deepEqual(
      normalizeTypographyPreferences({ uiFontSize: invalid, codeFontSize: invalid }),
      { uiFontSize: 14, codeFontSize: 12 },
    )
  }
})

test('only complete in-range integer edits can commit numeric font inputs', () => {
  for (const value of ['', ' ', '-', '1e', '11.5', '17', '10', 'Infinity'])
    assert.equal(parseFontSizeDraft(value, 'ui'), null)
  assert.equal(parseFontSizeDraft('11', 'ui'), 11)
  assert.equal(parseFontSizeDraft('16', 'ui'), 16)
  assert.equal(parseFontSizeDraft('8', 'code'), 8)
  assert.equal(parseFontSizeDraft('24', 'code'), 24)
  assert.equal(parseFontSizeDraft('7', 'code'), null)
  assert.equal(parseFontSizeDraft('25', 'code'), null)
})

test('numeric UI size scales the UI roles without changing the 16px rem base', () => {
  // 14px 基准阶梯 12/13/14/16/20/24/28：xl 桌面页 h1、2xl 手机 h1、3xl 数字主值。
  assert.deepEqual(uiTextSizes(14), [12, 13, 14, 16, 20, 24, 28])
  assert.deepEqual(uiTextSizes(11), [9, 10, 11, 13, 16, 19, 22])
  assert.deepEqual(uiTextSizes(16), [14, 15, 16, 18, 23, 27, 32])
  assert.deepEqual(uiTextSizes(14), uiTextSizes('default'))
})

test('the existing page-state key persists independent sizes and migrates older UI state', async () => {
  const values = new Map([
    ['pisper-ui', JSON.stringify({ version: 1, state: { fontScale: 'large', theme: 'dark' } })],
  ])
  const previous = {
    window: Object.getOwnPropertyDescriptor(globalThis, 'window'),
    localStorage: Object.getOwnPropertyDescriptor(globalThis, 'localStorage'),
  }
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage })
  try {
    const { useUiStore: store } = await import('../../src/stores/ui-store.ts')
    assert.equal(store.getState().uiFontSize, 14.5)
    assert.equal(store.getState().codeFontSize, 12)
    assert.equal(store.getState().theme, 'dark')
    store.getState().setUiFontSize(16)
    store.getState().setCodeFontSize(24)
    await store.persist.rehydrate()
    assert.equal(store.getState().uiFontSize, 16)
    assert.equal(store.getState().codeFontSize, 24)
    assert.equal(JSON.parse(values.get('pisper-ui')).version, 3)
    store.getState().resetAppearance()
    assert.equal(store.getState().uiFontSize, 14)
    assert.equal(store.getState().codeFontSize, 12)
    assert.deepEqual([...values.keys()], ['pisper-ui'])
  } finally {
    for (const [key, descriptor] of Object.entries(previous)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
      else delete globalThis[key]
    }
  }
})
