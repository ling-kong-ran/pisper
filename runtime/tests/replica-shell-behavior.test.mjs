import assert from 'node:assert/strict'
import test from 'node:test'
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH } from '../../src/lib/sidebar-width.ts'
import { uiTextSizes } from '../../src/app/ui-preferences.ts'

test('sidebar restores a valid preference while reserving space for the conversation', () => {
  assert.equal(DEFAULT_SIDEBAR_WIDTH, 340)
  assert.equal(clampSidebarWidth(undefined, 1200), 340)
  assert.equal(clampSidebarWidth(NaN, 1200), 340)
  assert.equal(clampSidebarWidth(Infinity, 1200), 340)
  assert.equal(clampSidebarWidth(100, 1200), 240)
  assert.equal(clampSidebarWidth(1000, 1200), 520)
  assert.equal(clampSidebarWidth(480, 650), 410)
  assert.equal(clampSidebarWidth(480, 400), 240)
  // 窄窗只约束当前渲染；恢复宽窗时仍使用用户保存的偏好。
  const preferred = 480
  assert.equal(clampSidebarWidth(preferred, 650), 410)
  assert.equal(clampSidebarWidth(preferred, 1200), preferred)
})

test('UI font preferences round their scale without changing content and code font roles', () => {
  assert.deepEqual(uiTextSizes('default'), [12, 13, 14, 16, 28, 36, 48])
  assert.deepEqual(uiTextSizes('small'), [12, 13, 14, 15, 27, 35, 46])
  assert.deepEqual(uiTextSizes('large'), [12, 13, 15, 17, 29, 37, 50])
  assert.deepEqual(uiTextSizes('retired-preference'), uiTextSizes('default'))
})
